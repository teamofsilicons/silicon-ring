//! The application controller: session, realtime events, call state and media.
//!
//! Everything here runs on the GTK main thread. Network and media work is handed to the Tokio
//! runtime through `runtime::run`, and results come back to this thread to touch the UI.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::{gio, glib};
use ring_core::events::Update;
use ring_core::media::{CallMedia, MediaEvent, MediaOptions, MediaState};
use ring_core::{CallFilter, Client, VoicemailFilter};
use ring_proto::{
    AcceptCall, Call, CallState, CarbonLogin, CreateCall, DeclineCall, Device, DeviceUpdate, Event,
    Invitation, InvitationState, Me, MediaGrant, Meta, ParticipantState, Platform,
    RingingStoppedReason, SettingsPatch, Voicemail, codes,
};

use crate::config::{Appearance, Config};
use crate::notify::{Note, NoteKind, Notifier};
use crate::sound::{Cue, Sounds};
use crate::ui::incoming::IncomingWindow;
use crate::ui::widgets;
use crate::ui::window::{MainWindow, Page};
use crate::{VERSION, autostart, runtime, secret, style};

/// Start-up options from the command line and environment.
#[derive(Clone, Debug)]
pub struct Options {
    pub api_url: String,
    pub background: bool,
}

/// The call this device is taking part in.
struct ActiveCall {
    call: Call,
    /// The LiveKit identity this device publishes as.
    identity: Option<String>,
    muted: bool,
    /// Set while a rejoin is in flight so media drops do not stack retries.
    rejoining: bool,
    /// Rejoins since the media last connected, for backing off.
    rejoins: u32,
}

/// What a rejoin found when it looked at the call again.
enum Rejoin {
    Media(ring_proto::CallWithMedia),
    MovedTo(Call, String),
    Over(Call),
}

pub struct Inner {
    gtk: adw::Application,
    options: Options,
    config: RefCell<Config>,
    client: RefCell<Client>,
    me: RefCell<Option<Me>>,
    meta: RefCell<Option<Meta>>,
    window: RefCell<Option<MainWindow>>,
    hold: RefCell<Option<gio::ApplicationHoldGuard>>,
    /// Bumped on every sign-in/sign-out so stale realtime and media callbacks are ignored.
    generation: Cell<u64>,
    stop_events: RefCell<Option<tokio::sync::oneshot::Sender<()>>>,
    stop_sign_in: RefCell<Option<tokio::sync::oneshot::Sender<()>>>,
    online: Cell<bool>,
    active: RefCell<Option<ActiveCall>>,
    media: RefCell<Option<CallMedia>>,
    media_generation: Cell<u64>,
    incoming: RefCell<HashMap<String, IncomingWindow>>,
    /// Invitations already answered, declined or put away on this device, so reconnecting does
    /// not ring them again.
    handled: RefCell<HashSet<String>>,
    /// Live calls (ringing, active, voicemail) this account is part of, for the handoff banner.
    live: RefCell<HashMap<String, Call>>,
    sounds: Sounds,
    notifier: Rc<Notifier>,
    refresh_pending: Cell<bool>,
    /// `--new-call` (or the desktop action) arrived before the session was restored.
    dialer_pending: Cell<bool>,
}

#[derive(Clone)]
pub struct App(Rc<Inner>);

thread_local! {
    static INSTANCE: RefCell<Option<App>> = const { RefCell::new(None) };
}

fn is_auth_error(err: &ring_core::Error) -> bool {
    matches!(err, ring_core::Error::NotSignedIn)
        || matches!(err.code(), codes::UNAUTHENTICATED | codes::SESSION_EXPIRED)
        || err.status() == Some(401)
}

/// A friendly default device name: the pretty hostname when set, else the hostname.
fn device_name() -> String {
    let pretty = std::fs::read_to_string("/etc/machine-info")
        .ok()
        .and_then(|text| {
            text.lines()
                .find_map(|line| line.strip_prefix("PRETTY_HOSTNAME="))
                .map(|v| v.trim().trim_matches('"').to_string())
        });
    let name = pretty
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| glib::host_name().to_string());
    name.chars().take(ring_proto::limits::DEVICE_NAME).collect()
}

/// The id of the system default device in a listing, for "System default" during a call.
fn default_device(
    devices: ring_core::Result<Vec<ring_core::media::AudioDevice>>,
) -> Option<String> {
    devices
        .ok()?
        .into_iter()
        .find(|d| d.is_default)
        .map(|d| d.id)
}

impl App {
    pub fn new(gtk: &adw::Application, options: Options) -> Result<App, ring_core::Error> {
        let user_agent = format!("ring-desktop/{VERSION} (linux)");
        let client = Client::new(&options.api_url, None, &user_agent)?;
        let app = App(Rc::new(Inner {
            gtk: gtk.clone(),
            options,
            config: RefCell::new(Config::load()),
            client: RefCell::new(client),
            me: RefCell::new(None),
            meta: RefCell::new(None),
            window: RefCell::new(None),
            hold: RefCell::new(None),
            generation: Cell::new(0),
            stop_events: RefCell::new(None),
            stop_sign_in: RefCell::new(None),
            online: Cell::new(false),
            active: RefCell::new(None),
            media: RefCell::new(None),
            media_generation: Cell::new(0),
            incoming: RefCell::new(HashMap::new()),
            handled: RefCell::new(HashSet::new()),
            live: RefCell::new(HashMap::new()),
            sounds: Sounds::new(),
            notifier: Rc::new(Notifier::default()),
            refresh_pending: Cell::new(false),
            dialer_pending: Cell::new(false),
        }));
        INSTANCE.with(|i| i.replace(Some(app.clone())));
        Ok(app)
    }

    pub fn get() -> App {
        Self::try_get().expect("the application controller exists")
    }

    pub fn try_get() -> Option<App> {
        INSTANCE.with(|i| i.borrow().clone())
    }

    pub fn client(&self) -> Client {
        self.0.client.borrow().clone()
    }

    pub fn me(&self) -> Option<Me> {
        self.0.me.borrow().clone()
    }

    pub fn my_uuid(&self) -> String {
        self.0
            .me
            .borrow()
            .as_ref()
            .map(|m| m.account.uuid.clone())
            .unwrap_or_default()
    }

    fn my_device_id(&self) -> String {
        self.0
            .me
            .borrow()
            .as_ref()
            .map(|m| m.device.device_id.clone())
            .unwrap_or_default()
    }

    pub fn meta(&self) -> Option<Meta> {
        self.0.meta.borrow().clone()
    }

    pub fn config(&self) -> Config {
        self.0.config.borrow().clone()
    }

    pub fn update_config(&self, f: impl FnOnce(&mut Config)) {
        let mut config = self.0.config.borrow_mut();
        f(&mut config);
        config.save();
    }

    pub fn window(&self) -> MainWindow {
        if let Some(window) = self.0.window.borrow().as_ref() {
            return window.clone();
        }
        let window = MainWindow::new(&self.0.gtk);
        self.0.window.replace(Some(window.clone()));
        window
    }

    pub fn present(&self) {
        self.window().window.present();
    }

    pub fn toast(&self, text: &str) {
        self.window().toast(text);
    }

    pub fn error(&self, err: &ring_core::Error) {
        tracing::warn!("request failed: {err}");
        self.toast(&widgets::error_text(err));
    }

    pub fn has_active_call(&self) -> bool {
        self.0.active.borrow().is_some()
    }

    // ---- start-up and session -------------------------------------------------------------------

    /// Called once from `startup`: applies preferences and restores the stored session.
    pub fn start(&self) {
        style::apply_appearance(self.0.config.borrow().appearance);
        crate::notify::warm_up();
        crate::ui::player::clear_cache();
        let window = self.window();
        window.show(Page::Loading);
        let api_url = self.0.options.api_url.clone();
        let app = self.clone();
        glib::spawn_future_local(async move {
            let token = runtime::run(async move { secret::load(&api_url).await }).await;
            // Started at login without a session there is nothing to listen for, and popping
            // up a sign-in window unasked would be rude. A window opened since then stays.
            let hidden = !app.window().window.is_visible();
            match token {
                Some(token) => app.resume(token),
                None if app.background_start() && hidden => {
                    tracing::info!("not signed in; nothing to do in the background");
                    app.0.gtk.quit();
                }
                None => app.show_sign_in(None),
            }
        });
    }

    pub fn background_start(&self) -> bool {
        self.0.options.background
    }

    fn show_sign_in(&self, error: Option<&str>) {
        let window = self.window();
        window.signin.set_waiting(false);
        window.signin.set_error(error);
        window.show(Page::SignIn);
        if error.is_some() {
            window.window.present();
        }
    }

    /// Validates a stored token and enters the signed-in state, retrying while offline.
    fn resume(&self, token: String) {
        let generation = self.0.generation.get() + 1;
        self.0.generation.set(generation);
        let client = self.client().with_session(Some(token));
        self.0.client.replace(client.clone());
        self.window()
            .loading
            .set_status("Connecting to Ring…", None);
        self.window().show(Page::Loading);
        let app = self.clone();
        glib::spawn_future_local(async move {
            let mut delay = 2;
            loop {
                let c = client.clone();
                let result = runtime::run(async move { c.me().await }).await;
                if app.0.generation.get() != generation {
                    return;
                }
                match result {
                    Ok(me) => {
                        app.signed_in(me);
                        return;
                    }
                    Err(err) if is_auth_error(&err) => {
                        app.forget_session().await;
                        app.show_sign_in(Some("Your session ended. Sign in again."));
                        return;
                    }
                    Err(err) => {
                        app.window().loading.set_status(
                            "Can't reach Ring",
                            Some(&format!("{} Retrying…", widgets::error_text(&err))),
                        );
                        glib::timeout_future_seconds(delay).await;
                        delay = (delay * 2).min(30);
                    }
                }
            }
        });
    }

    fn signed_in(&self, me: Me) {
        tracing::info!(
            "signed in as {} on device {}",
            me.account.id,
            me.device.device_id
        );
        self.0.me.replace(Some(me.clone()));
        self.apply_hold();
        if self.0.config.borrow().open_at_login_initialized {
            autostart::request_keep_running(self.open_at_login());
        } else {
            // Ring listens for calls from login onwards, like a phone; Settings can turn it off.
            self.update_config(|c| c.open_at_login_initialized = true);
            self.set_open_at_login(true);
        }

        let window = self.window();
        window.home.settings.set_me(&me);
        window.show(Page::Home);

        // Describe this device to the server (platform, version, a name the first time).
        let client = self.client();
        let update = DeviceUpdate {
            name: me.device.name.is_none().then(device_name),
            platform: Some(Platform::Linux),
            app_version: Some(VERSION.to_string()),
            ..Default::default()
        };
        let app = self.clone();
        glib::spawn_future_local(async move {
            match runtime::run(async move { client.update_current_device(&update).await }).await {
                Ok(device) => app.device_changed(device),
                Err(err) => tracing::warn!("device update failed: {err}"),
            }
        });

        self.load_meta();
        self.start_events();
        self.refresh_all();
        if self.0.dialer_pending.replace(false) {
            self.present();
            crate::ui::dialer::present(crate::ui::dialer::Mode::Call);
        }
    }

    /// The "New call" action: opens the dialer now, or once the stored session is restored.
    pub fn new_call(&self) {
        if self.0.me.borrow().is_some() {
            self.present();
            crate::ui::dialer::present(crate::ui::dialer::Mode::Call);
        } else {
            self.0
                .dialer_pending
                .set(self.window().current_page() == Page::Loading);
            self.present();
        }
    }

    fn load_meta(&self) {
        let client = self.client();
        let app = self.clone();
        glib::spawn_future_local(async move {
            if let Ok(meta) = runtime::run(async move { client.meta().await }).await {
                app.window().home.settings.set_voices(&meta.voices);
                app.0.meta.replace(Some(meta));
            }
        });
    }

    async fn forget_session(&self) {
        self.stop_events();
        self.0.generation.set(self.0.generation.get() + 1);
        let api_url = self.0.options.api_url.clone();
        runtime::run(async move { secret::clear(&api_url).await }).await;
        self.end_local_call(None, false);
        for (_, window) in self.0.incoming.borrow_mut().drain() {
            window.close();
        }
        self.0.sounds.stop_ringtone();
        self.0.me.replace(None);
        self.0.live.borrow_mut().clear();
        self.0.handled.borrow_mut().clear();
        let anonymous = self.client().with_session(None);
        self.0.client.replace(anonymous);
        self.0.hold.replace(None);
        self.window().home.reset();
    }

    pub fn sign_in(&self) {
        let window = self.window();
        window.signin.set_error(None);
        window.signin.set_waiting(true);
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
        self.0.stop_sign_in.replace(Some(stop_tx));
        let client = self.client().with_session(None);
        let app = self.clone();
        glib::spawn_future_local(async move {
            let result = app.run_sign_in(client, stop_rx).await;
            app.0.stop_sign_in.replace(None);
            match result {
                Ok(Some(token)) => app.resume(token),
                Ok(None) => app.show_sign_in(None),
                Err(message) => app.show_sign_in(Some(&message)),
            }
        });
    }

    pub fn cancel_sign_in(&self) {
        if let Some(stop) = self.0.stop_sign_in.take() {
            let _ = stop.send(());
        }
    }

    /// The desktop flow from udd/accounts.md: PKCE, the system browser and a loopback redirect.
    /// Returns the new session token, `None` when cancelled.
    async fn run_sign_in(
        &self,
        client: Client,
        stop: tokio::sync::oneshot::Receiver<()>,
    ) -> Result<Option<String>, String> {
        use ring_core::auth::{Callback, LoopbackReceiver, Pkce, authorize_url};

        let c = client.clone();
        let prepared = runtime::run(async move {
            let meta = c.meta().await?;
            let receiver = LoopbackReceiver::bind().await?;
            Ok::<_, ring_core::Error>((meta, receiver))
        })
        .await;
        let (meta, receiver) = prepared.map_err(|err| widgets::error_text(&err))?;
        let pkce = Pkce::new();
        let redirect_uri = receiver.redirect_uri.clone();
        let url = authorize_url(&meta.accounts_url, &redirect_uri, &pkce);
        self.0.meta.replace(Some(meta));

        let window = self.window();
        if let Err(err) = gtk::UriLauncher::new(&url)
            .launch_future(Some(&window.window))
            .await
        {
            tracing::warn!("could not open the browser: {err}");
            window.signin.show_manual_link(&url);
        }

        let callback = runtime::run(async move {
            tokio::select! {
                result = receiver.wait() => Some(result),
                _ = stop => None,
                _ = tokio::time::sleep(Duration::from_secs(600)) => {
                    Some(Err(ring_core::Error::Input("Sign-in timed out. Try again.".into())))
                }
            }
        })
        .await;
        window.window.present();

        let (code, state) = match callback {
            None => return Ok(None),
            Some(Err(err)) => return Err(widgets::error_text(&err)),
            Some(Ok(Callback::Error { error, description })) => {
                if error == "access_denied" {
                    return Ok(None);
                }
                return Err(description.unwrap_or_else(|| format!("Sign-in failed ({error}).")));
            }
            Some(Ok(Callback::Code { code, state })) => (code, state),
        };
        if state != pkce.state {
            return Err("Sign-in could not be verified. Try again.".into());
        }

        let login = CarbonLogin {
            code,
            code_verifier: pkce.verifier,
            redirect_uri,
            label: Some(device_name()),
        };
        let session = runtime::run(async move { client.login_carbon(&login).await })
            .await
            .map_err(|err| widgets::error_text(&err))?;
        let api_url = self.0.options.api_url.clone();
        let token = session.session_token.clone();
        // The server session exists now, so a storage failure must not throw it away.
        if let Err(err) = runtime::run(async move { secret::store(&api_url, &token).await }).await
        {
            tracing::warn!("{err}");
            self.toast("Signed in, but Ring couldn't save the session. You'll sign in again next time.");
        }
        Ok(Some(session.session_token))
    }

    pub fn sign_out(&self) {
        let client = self.client();
        // The server answers the sign-out with `session.revoked` on the realtime connection;
        // stop listening first so that is not reported as an unexpected sign-out.
        self.stop_events();
        self.0.generation.set(self.0.generation.get() + 1);
        let app = self.clone();
        glib::spawn_future_local(async move {
            if let Some(ringid) = app.active_ringid() {
                let c = client.clone();
                let _ = runtime::run(async move { c.leave(&ringid).await }).await;
            }
            if let Err(err) = runtime::run(async move { client.logout().await }).await {
                tracing::warn!("server sign-out failed: {err}");
            }
            app.forget_session().await;
            app.show_sign_in(None);
        });
    }

    // ---- background behaviour ---------------------------------------------------------------------

    /// While signed in and allowed to, hold the application so it keeps listening with no window.
    fn apply_hold(&self) {
        let keep = self.0.config.borrow().keep_running && self.0.me.borrow().is_some();
        if keep {
            if self.0.hold.borrow().is_none() {
                self.0.hold.replace(Some(self.0.gtk.hold()));
            }
        } else {
            self.0.hold.replace(None);
        }
    }

    pub fn set_keep_running(&self, keep: bool) {
        self.update_config(|c| c.keep_running = keep);
        self.apply_hold();
        if keep {
            autostart::request_keep_running(self.open_at_login());
        }
    }

    pub fn open_at_login(&self) -> bool {
        if autostart::sandboxed() {
            self.0.config.borrow().open_at_login
        } else {
            autostart::entry_enabled()
        }
    }

    pub fn set_open_at_login(&self, enabled: bool) {
        let app = self.clone();
        glib::spawn_future_local(async move {
            match autostart::set_open_at_login(enabled).await {
                Ok(state) => {
                    app.update_config(|c| c.open_at_login = state);
                    if state != enabled {
                        app.toast("Ring was not allowed to start at login.");
                    }
                    app.window().home.settings.set_open_at_login(state);
                }
                Err(err) => {
                    app.toast(&err);
                    app.window()
                        .home
                        .settings
                        .set_open_at_login(app.open_at_login());
                }
            }
        });
    }

    pub fn set_appearance(&self, appearance: Appearance) {
        self.update_config(|c| c.appearance = appearance);
        style::apply_appearance(appearance);
    }

    /// Whether closing the main window should hide it instead of quitting.
    pub fn close_to_background(&self) -> bool {
        let keep = self.0.config.borrow().keep_running && self.0.me.borrow().is_some();
        if keep && !self.0.config.borrow().told_about_background {
            self.update_config(|c| c.told_about_background = true);
            self.0.notifier.show(
                &self.0.gtk,
                Note {
                    id: "background".into(),
                    title: "Ring is still listening for calls".into(),
                    body: "Turn this off in Settings under \u{201c}Keep running in the background\u{201d}.".into(),
                    kind: NoteKind::Info,
                    target: String::new(),
                },
            );
        }
        keep
    }

    pub fn quit(&self) {
        let app = self.clone();
        glib::spawn_future_local(async move {
            if let Some(ringid) = app.active_ringid() {
                let client = app.client();
                let _ = runtime::run(async move { client.leave(&ringid).await }).await;
            }
            app.drop_media();
            app.0.hold.replace(None);
            app.0.gtk.quit();
        });
    }

    // ---- realtime events --------------------------------------------------------------------------

    fn stop_events(&self) {
        if let Some(stop) = self.0.stop_events.take() {
            let _ = stop.send(());
        }
    }

    fn start_events(&self) {
        self.stop_events();
        let generation = self.0.generation.get();
        let (tx, rx) = async_channel::unbounded::<Result<Update, ring_core::Error>>();
        let (stop_tx, mut stop_rx) = tokio::sync::oneshot::channel::<()>();
        self.0.stop_events.replace(Some(stop_tx));
        let client = self.client();
        runtime::spawn(async move {
            let mut subscription = match client.subscribe().await {
                Ok(subscription) => subscription,
                Err(err) => {
                    let _ = tx.send(Err(err)).await;
                    return;
                }
            };
            loop {
                tokio::select! {
                    _ = &mut stop_rx => break,
                    update = subscription.next() => match update {
                        Some(update) => {
                            if tx.send(Ok(update)).await.is_err() {
                                break;
                            }
                        }
                        None => break,
                    },
                }
            }
            subscription.close();
        });
        let app = self.clone();
        glib::spawn_future_local(async move {
            while let Ok(message) = rx.recv().await {
                if app.0.generation.get() != generation {
                    break;
                }
                match message {
                    Ok(update) => app.on_update(update),
                    Err(err) => {
                        tracing::warn!("realtime connection failed: {err}");
                        if is_auth_error(&err) {
                            app.on_update(Update::SignedOut);
                        } else {
                            app.set_online(false);
                            // ring-core retries on its own once connected; a failure to start is retried here.
                            glib::timeout_future_seconds(5).await;
                            if app.0.generation.get() == generation {
                                app.start_events();
                            }
                        }
                        break;
                    }
                }
            }
        });
    }

    fn set_online(&self, online: bool) {
        self.0.online.set(online);
        self.window().home.set_offline(!online);
    }

    fn on_update(&self, update: Update) {
        match update {
            Update::Connected => {
                self.set_online(true);
                self.reconcile();
            }
            Update::Disconnected {
                reason,
                retry_in_ms,
            } => {
                tracing::info!("realtime disconnected ({reason}); retrying in {retry_in_ms} ms");
                self.set_online(false);
            }
            Update::SignedOut => {
                let app = self.clone();
                glib::spawn_future_local(async move {
                    app.forget_session().await;
                    app.show_sign_in(Some("You were signed out of Ring on this device."));
                });
            }
            Update::Event(event) => self.on_event(event),
        }
    }

    fn on_event(&self, event: Event) {
        match event {
            Event::Ready { .. } | Event::Pong {} | Event::DelegationCreated { .. } => {}
            Event::CallIncoming { call, invitation } => {
                self.call_changed(call.clone());
                self.ring(&call, &invitation);
            }
            Event::CallUpdated { call } | Event::CallEnded { call } => self.call_changed(call),
            Event::CallRingingStopped {
                ringid,
                invitation_id,
                reason,
            } => self.ringing_stopped(&ringid, &invitation_id, reason),
            Event::CallHandoff { ringid, device_id } => self.handoff(&ringid, &device_id),
            Event::TranscriptEntry { ringid, entry } => {
                if self.active_ringid().as_deref() == Some(ringid.as_str()) {
                    self.window().call.add_entry(&entry);
                }
            }
            Event::TranscriptPartial {
                ringid,
                speaker,
                segment_id,
                text,
            } => {
                if self.active_ringid().as_deref() == Some(ringid.as_str()) {
                    self.window().call.partial(&speaker, &segment_id, &text);
                }
            }
            Event::VoicemailReceived { voicemail } => {
                self.notify_voicemail(&voicemail);
                self.window().home.voicemail.upsert(voicemail);
                self.update_unread();
            }
            Event::VoicemailUpdated { voicemail } => {
                if voicemail.to.uuid == self.my_uuid() {
                    self.window().home.voicemail.upsert(voicemail);
                    self.update_unread();
                }
            }
            Event::DeviceUpdated { device } => self.device_changed(device),
            Event::SessionRevoked {} => self.on_update(Update::SignedOut),
        }
    }

    /// After (re)connecting: catch up on live calls and history missed while offline.
    fn reconcile(&self) {
        let client = self.client();
        let app = self.clone();
        let filter = CallFilter {
            states: vec![CallState::Ringing, CallState::Active, CallState::Voicemail],
            limit: Some(50),
            ..Default::default()
        };
        glib::spawn_future_local(async move {
            match runtime::run(async move { client.calls(&filter).await }).await {
                Ok(page) => {
                    let live: Vec<String> = page.items.iter().map(|c| c.ringid.clone()).collect();
                    // Calls that ended while offline.
                    let stale: Vec<String> = app
                        .0
                        .live
                        .borrow()
                        .keys()
                        .filter(|k| !live.contains(k))
                        .cloned()
                        .collect();
                    for ringid in stale {
                        app.0.live.borrow_mut().remove(&ringid);
                        app.close_incoming_for_call(&ringid);
                    }
                    if let Some(ringid) = app.active_ringid() {
                        if !live.contains(&ringid) {
                            let client = app.client();
                            let id = ringid.clone();
                            let ended = runtime::run(async move { client.call(&id).await })
                                .await
                                .ok();
                            app.end_local_call(ended, true);
                        }
                    }
                    let me = app.my_uuid();
                    for call in page.items {
                        app.call_changed(call.clone());
                        if let Some(invitation) = call.ringing_invitation_for(&me).cloned() {
                            if !invitation.silenced {
                                app.ring(&call, &invitation);
                            }
                        }
                    }
                }
                Err(err) => tracing::warn!("reconcile failed: {err}"),
            }
        });
        self.refresh_all();
    }

    pub fn refresh_all(&self) {
        self.refresh_recents();
        self.refresh_voicemail();
        self.refresh_settings();
    }

    /// Coalesces history refreshes triggered by bursts of call events.
    fn schedule_recents_refresh(&self) {
        if self.0.refresh_pending.replace(true) {
            return;
        }
        let app = self.clone();
        glib::timeout_add_local_once(Duration::from_millis(600), move || {
            app.0.refresh_pending.set(false);
            app.refresh_recents();
        });
    }

    pub fn refresh_recents(&self) {
        if self.0.me.borrow().is_none() {
            return;
        }
        let client = self.client();
        let app = self.clone();
        let filter = CallFilter {
            limit: Some(100),
            ..Default::default()
        };
        glib::spawn_future_local(async move {
            match runtime::run(async move { client.calls(&filter).await }).await {
                Ok(page) => app
                    .window()
                    .home
                    .recents
                    .set_calls(page.items, &app.my_uuid()),
                Err(err) => {
                    tracing::warn!("loading recents failed: {err}");
                    app.window()
                        .home
                        .recents
                        .set_error(&widgets::error_text(&err));
                }
            }
        });
    }

    pub fn refresh_voicemail(&self) {
        if self.0.me.borrow().is_none() {
            return;
        }
        let client = self.client();
        let app = self.clone();
        let filter = VoicemailFilter {
            limit: Some(100),
            ..Default::default()
        };
        glib::spawn_future_local(async move {
            match runtime::run(async move { client.voicemails(&filter).await }).await {
                Ok(page) => {
                    app.window().home.voicemail.set_voicemails(page.items);
                    app.update_unread();
                }
                Err(err) => {
                    tracing::warn!("loading voicemail failed: {err}");
                    app.window()
                        .home
                        .voicemail
                        .set_error(&widgets::error_text(&err));
                }
            }
        });
    }

    fn update_unread(&self) {
        let window = self.window();
        let unread = window.home.voicemail.unread_count();
        window.home.set_unread(unread);
    }

    pub fn refresh_settings(&self) {
        if self.0.me.borrow().is_none() {
            return;
        }
        let client = self.client();
        let app = self.clone();
        glib::spawn_future_local(async move {
            let c = client.clone();
            match runtime::run(async move { c.settings().await }).await {
                Ok(settings) => app.window().home.settings.set_settings(&settings),
                Err(err) => tracing::warn!("loading settings failed: {err}"),
            }
            match runtime::run(async move { client.devices().await }).await {
                Ok(devices) => app.window().home.settings.set_devices(&devices),
                Err(err) => tracing::warn!("loading devices failed: {err}"),
            }
        });
    }

    fn device_changed(&self, device: Device) {
        if device.current || device.device_id == self.my_device_id() {
            if let Some(me) = self.0.me.borrow_mut().as_mut() {
                me.device = device.clone();
            }
            self.window().home.settings.set_this_device(&device);
        }
        self.window().home.settings.upsert_device(&device);
    }

    pub fn rename_device(&self, name: String) {
        let client = self.client();
        let app = self.clone();
        let update = DeviceUpdate {
            name: Some(name),
            ..Default::default()
        };
        glib::spawn_future_local(async move {
            match runtime::run(async move { client.update_current_device(&update).await }).await {
                Ok(device) => {
                    app.device_changed(device);
                    app.toast("Device renamed");
                }
                Err(err) => app.error(&err),
            }
        });
    }

    pub fn set_ring_enabled(&self, enabled: bool) {
        let client = self.client();
        let app = self.clone();
        let update = DeviceUpdate {
            ring_enabled: Some(enabled),
            ..Default::default()
        };
        glib::spawn_future_local(async move {
            match runtime::run(async move { client.update_current_device(&update).await }).await {
                Ok(device) => app.device_changed(device),
                Err(err) => {
                    app.error(&err);
                    if let Some(me) = app.me() {
                        app.window().home.settings.set_this_device(&me.device);
                    }
                }
            }
        });
    }

    pub fn revoke_device(&self, device_id: String) {
        let client = self.client();
        let app = self.clone();
        glib::spawn_future_local(async move {
            let id = device_id.clone();
            match runtime::run(async move { client.revoke_device(&id).await }).await {
                Ok(()) => {
                    app.window().home.settings.remove_device(&device_id);
                    app.toast("Device signed out");
                }
                Err(err) => app.error(&err),
            }
        });
    }

    /// `PATCH /v1/settings`, then show what the server stored.
    pub fn update_settings(&self, patch: SettingsPatch, done: &'static str) {
        let client = self.client();
        let app = self.clone();
        glib::spawn_future_local(async move {
            match runtime::run(async move { client.update_settings(&patch).await }).await {
                Ok(settings) => {
                    app.window().home.settings.set_settings(&settings);
                    if !done.is_empty() {
                        app.toast(done);
                    }
                }
                Err(err) => {
                    app.error(&err);
                    app.refresh_settings();
                }
            }
        });
    }

    // ---- incoming calls ---------------------------------------------------------------------------

    fn ring(&self, call: &Call, invitation: &Invitation) {
        let Some(me) = self.me() else { return };
        if invitation.to.uuid != me.account.uuid
            || invitation.state != InvitationState::Ringing
            || invitation.silenced
        {
            return;
        }
        if !me.device.ring_enabled || self.0.handled.borrow().contains(&invitation.invitation_id) {
            return;
        }
        if let Some(window) = self.0.incoming.borrow().get(&invitation.invitation_id) {
            window.update(call);
            return;
        }
        let in_call = self.has_active_call();
        let window = IncomingWindow::new(&self.0.gtk, call, invitation, &me.account.uuid, in_call);
        window.present();
        self.0
            .incoming
            .borrow_mut()
            .insert(invitation.invitation_id.clone(), window);
        self.0.sounds.start_ringtone();

        let caller = &invitation.from;
        let others: Vec<String> = call
            .joined()
            .filter(|p| p.account.uuid != caller.uuid && p.account.uuid != me.account.uuid)
            .map(|p| p.account.label().to_string())
            .collect();
        let body = if others.is_empty() {
            caller.id.clone()
        } else {
            format!("{} · with {}", caller.id, others.join(", "))
        };
        self.0.notifier.show(
            &self.0.gtk,
            Note {
                id: format!("call-{}", invitation.invitation_id),
                title: format!("{} is calling", caller.label()),
                body,
                kind: NoteKind::IncomingCall,
                target: invitation.invitation_id.clone(),
            },
        );

        // Ringing ends after 60 s on the server; make sure the local UI never outlives it.
        let remaining = (invitation.expires_at.millis() - ring_proto::Time::now().millis())
            .clamp(1_000, 70_000) as u64;
        let app = self.clone();
        let invitation_id = invitation.invitation_id.clone();
        glib::timeout_add_local_once(Duration::from_millis(remaining + 2_000), move || {
            app.dismiss_incoming(&invitation_id);
        });
    }

    fn incoming_call_ids(&self, invitation_id: &str) -> Option<(String, String)> {
        self.0
            .incoming
            .borrow()
            .get(invitation_id)
            .map(|w| (w.ringid(), invitation_id.to_string()))
    }

    /// Closes the ringing UI for one invitation on this device only.
    pub fn dismiss_incoming(&self, invitation_id: &str) {
        self.0
            .handled
            .borrow_mut()
            .insert(invitation_id.to_string());
        let window = self.0.incoming.borrow_mut().remove(invitation_id);
        if let Some(window) = window {
            window.close();
        }
        self.0
            .notifier
            .withdraw(&self.0.gtk, &format!("call-{invitation_id}"));
        if self.0.incoming.borrow().is_empty() {
            self.0.sounds.stop_ringtone();
        }
    }

    fn close_incoming_for_call(&self, ringid: &str) {
        let ids: Vec<String> = self
            .0
            .incoming
            .borrow()
            .iter()
            .filter(|(_, w)| w.ringid() == ringid)
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            self.dismiss_incoming(&id);
        }
    }

    fn ringing_stopped(&self, ringid: &str, invitation_id: &str, reason: RingingStoppedReason) {
        let was_ringing = self.0.incoming.borrow().contains_key(invitation_id);
        let caller = self
            .0
            .incoming
            .borrow()
            .get(invitation_id)
            .map(|w| w.caller());
        self.dismiss_incoming(invitation_id);
        if !was_ringing {
            return;
        }
        match reason {
            RingingStoppedReason::AcceptedElsewhere if !self.has_active_call() => {
                self.toast("Answered on another device")
            }
            RingingStoppedReason::Missed => {
                if let Some(caller) = caller {
                    self.0.notifier.show(
                        &self.0.gtk,
                        Note {
                            id: format!("missed-{ringid}"),
                            title: format!("Missed call from {}", caller.label()),
                            body: caller.id.clone(),
                            kind: NoteKind::MissedCall,
                            target: ringid.to_string(),
                        },
                    );
                }
            }
            _ => {}
        }
        self.schedule_recents_refresh();
    }

    /// Answers from the incoming window or a notification button.
    pub fn accept(&self, invitation_id: &str) {
        let Some((ringid, invitation_id)) = self.incoming_call_ids(invitation_id) else {
            self.toast("That call is no longer ringing.");
            return;
        };
        self.dismiss_incoming(&invitation_id);
        let app = self.clone();
        glib::spawn_future_local(async move {
            // Taking a second call ends the one in progress on this device.
            if let Some(current) = app.active_ringid() {
                let client = app.client();
                let _ = runtime::run(async move { client.leave(&current).await }).await;
                app.end_local_call(None, false);
            }
            let client = app.client();
            let req = AcceptCall {
                invitation_id: Some(invitation_id),
                ..Default::default()
            };
            let id = ringid.clone();
            match runtime::run(async move { client.accept(&id, &req).await }).await {
                Ok(answer) => app.begin_call(answer.call, answer.media),
                Err(err) if err.code() == codes::INVITATION_NOT_RINGING => {
                    app.toast("That call was answered on another device.")
                }
                Err(err) => app.error(&err),
            }
        });
    }

    pub fn decline(&self, invitation_id: &str, reason: Option<String>) {
        let Some((ringid, invitation_id)) = self.incoming_call_ids(invitation_id) else {
            return;
        };
        self.dismiss_incoming(&invitation_id);
        let client = self.client();
        let app = self.clone();
        let req = DeclineCall {
            invitation_id: Some(invitation_id),
            reason: reason.filter(|r| !r.trim().is_empty()),
            give_no_reason: false,
        };
        glib::spawn_future_local(async move {
            if let Err(err) = runtime::run(async move { client.decline(&ringid, &req).await }).await
            {
                if err.code() != codes::INVITATION_NOT_RINGING {
                    app.error(&err);
                }
            }
        });
    }

    /// Stops the ringing on every device without declining.
    pub fn silence(&self, invitation_id: &str) {
        let Some((ringid, invitation_id)) = self.incoming_call_ids(invitation_id) else {
            return;
        };
        self.dismiss_incoming(&invitation_id);
        let client = self.client();
        let app = self.clone();
        glib::spawn_future_local(async move {
            let id = invitation_id.clone();
            if let Err(err) =
                runtime::run(async move { client.silence(&ringid, Some(&id)).await }).await
            {
                if err.code() != codes::INVITATION_NOT_RINGING {
                    app.error(&err);
                }
            }
        });
    }

    pub fn show_incoming(&self, invitation_id: &str) {
        if let Some(window) = self.0.incoming.borrow().get(invitation_id) {
            window.present();
        }
    }

    // ---- outgoing and active calls ----------------------------------------------------------------

    pub fn active_call(&self) -> Option<Call> {
        self.0.active.borrow().as_ref().map(|a| a.call.clone())
    }

    pub fn active_ringid(&self) -> Option<String> {
        self.0
            .active
            .borrow()
            .as_ref()
            .map(|a| a.call.ringid.clone())
    }

    pub fn start_call(&self, to: String) {
        if self.has_active_call() {
            self.toast("Hang up before starting another call, or add them to this one.");
            self.window().show(Page::Call);
            return;
        }
        let client = self.client();
        let app = self.clone();
        self.window().call.show_dialing(&to);
        self.window().show(Page::Call);
        self.present();
        glib::spawn_future_local(async move {
            let req = CreateCall {
                to,
                ..Default::default()
            };
            match runtime::run(async move { client.create_call(&req, None).await }).await {
                Ok(created) => app.begin_call(created.call, created.media),
                Err(err) => {
                    app.error(&err);
                    app.window().show(Page::Home);
                }
            }
        });
    }

    /// Enters the call screen for `call` and joins its media on this device.
    fn begin_call(&self, call: Call, media: Option<MediaGrant>) {
        let me = self.my_uuid();
        self.0.active.replace(Some(ActiveCall {
            call: call.clone(),
            identity: media.as_ref().map(|m| m.identity.clone()),
            muted: false,
            rejoining: false,
            rejoins: 0,
        }));
        self.0
            .live
            .borrow_mut()
            .insert(call.ringid.clone(), call.clone());
        let window = self.window();
        window
            .call
            .load(&call, &me, media.as_ref().map(|m| m.identity.as_str()));
        window.show(Page::Call);
        window.home.set_call_elsewhere(None);
        window.home.set_return_to_call(Some(&call), &me);
        self.present();
        self.sync_call_sounds(None, &call);
        if let Some(grant) = media {
            self.connect_media(grant);
        }
        self.load_transcript(&call.ringid);
        if call.state == CallState::Ended {
            self.end_local_call(Some(call), true);
        }
    }

    fn load_transcript(&self, ringid: &str) {
        let client = self.client();
        let app = self.clone();
        let ringid = ringid.to_string();
        glib::spawn_future_local(async move {
            let id = ringid.clone();
            if let Ok(page) =
                runtime::run(async move { client.transcript(&id, 0, Some(200)).await }).await
            {
                if app.active_ringid().as_deref() == Some(ringid.as_str()) {
                    for entry in &page.items {
                        app.window().call.add_entry(entry);
                    }
                }
            }
        });
    }

    /// Ringback while an outgoing call rings; the connect tick when it is answered.
    fn sync_call_sounds(&self, previous: Option<CallState>, call: &Call) {
        let ringing_out =
            call.state == CallState::Ringing && call.created_by.uuid == self.my_uuid();
        if ringing_out {
            self.0.sounds.start_ringback();
        } else {
            self.0.sounds.stop_ringback();
        }
        if previous.is_some_and(|p| p != CallState::Active) && call.state == CallState::Active {
            self.0.sounds.play(Cue::Connected);
        }
    }

    fn call_changed(&self, call: Call) {
        let me = self.my_uuid();
        if call.state == CallState::Ended {
            self.0.live.borrow_mut().remove(&call.ringid);
            self.close_incoming_for_call(&call.ringid);
            self.schedule_recents_refresh();
        } else {
            self.0
                .live
                .borrow_mut()
                .insert(call.ringid.clone(), call.clone());
        }

        // Ringing windows follow the invitation and the roster.
        let windows: Vec<(String, bool)> = self
            .0
            .incoming
            .borrow()
            .iter()
            .filter(|(_, w)| w.ringid() == call.ringid)
            .map(|(id, _)| {
                (
                    id.clone(),
                    call.invitations.iter().any(|i| {
                        &i.invitation_id == id && i.state == InvitationState::Ringing && !i.silenced
                    }),
                )
            })
            .collect();
        for (invitation_id, still_ringing) in windows {
            if still_ringing {
                if let Some(window) = self.0.incoming.borrow().get(&invitation_id) {
                    window.update(&call);
                }
            } else {
                self.dismiss_incoming(&invitation_id);
            }
        }

        let previous = self
            .0
            .active
            .borrow_mut()
            .as_mut()
            .filter(|a| a.call.ringid == call.ringid)
            .map(|active| {
                let previous = active.call.state;
                active.call = call.clone();
                previous
            });
        if let Some(previous) = previous {
            let still_here = call.state != CallState::Ended
                && call
                    .participants
                    .iter()
                    .any(|p| p.account.uuid == me && p.state == ParticipantState::Joined);
            if still_here {
                self.window().call.update(&call);
                self.window().home.set_return_to_call(Some(&call), &me);
                self.sync_call_sounds(Some(previous), &call);
            } else {
                self.end_local_call(Some(call), true);
            }
        }
        self.update_elsewhere();
    }

    /// The banner offered when this account is on a call on another device.
    fn update_elsewhere(&self) {
        let me = self.my_uuid();
        let my_device = self.my_device_id();
        let here = self.active_ringid();
        let elsewhere = self.0.live.borrow().values().find_map(|call| {
            if call.state != CallState::Active || Some(&call.ringid) == here.as_ref() {
                return None;
            }
            let mine = call
                .participants
                .iter()
                .find(|p| p.account.uuid == me && p.state == ParticipantState::Joined)?;
            let device = mine.device.as_ref()?;
            (device.device_id != my_device).then(|| (call.clone(), device.clone()))
        });
        self.window().home.set_call_elsewhere(
            elsewhere
                .as_ref()
                .map(|(call, device)| (call, device, me.as_str())),
        );
    }

    pub fn hang_up(&self) {
        let Some(ringid) = self.active_ringid() else {
            self.window().show(Page::Home);
            return;
        };
        self.0.sounds.stop_ringback();
        self.drop_media();
        let client = self.client();
        let app = self.clone();
        glib::spawn_future_local(async move {
            let result = runtime::run(async move { client.leave(&ringid).await }).await;
            match result {
                Ok(call) => app.end_local_call(Some(call), true),
                Err(err) => {
                    if !matches!(
                        err.code(),
                        codes::CALL_ENDED | codes::NOT_PARTICIPANT | codes::NOT_FOUND
                    ) {
                        app.error(&err);
                    }
                    app.end_local_call(None, true);
                }
            }
        });
    }

    /// Leaves the call screen and releases media. `audible` plays the end tick.
    fn end_local_call(&self, call: Option<Call>, audible: bool) {
        let Some(active) = self.0.active.take() else {
            return;
        };
        self.0.sounds.stop_ringback();
        self.drop_media();
        if audible {
            self.0.sounds.play(Cue::Ended);
        }
        let window = self.window();
        window.home.set_return_to_call(None, "");
        let final_call = call.unwrap_or(active.call);
        window.call.show_ended(&final_call, &self.my_uuid());
        let app = self.clone();
        glib::timeout_add_local_once(Duration::from_millis(1800), move || {
            if app.active_ringid().is_none() && app.window().current_page() == Page::Call {
                app.window().show(Page::Home);
            }
        });
        self.schedule_recents_refresh();
        self.update_elsewhere();
    }

    pub fn invite(&self, to: String) {
        let Some(ringid) = self.active_ringid() else {
            return;
        };
        let client = self.client();
        let app = self.clone();
        glib::spawn_future_local(async move {
            match runtime::run(async move { client.invite(&ringid, &to, None).await }).await {
                Ok(invited) => {
                    app.toast(&format!("Ringing {}…", invited.invitation.to.label()));
                    app.call_changed(invited.call);
                }
                Err(err) => app.error(&err),
            }
        });
    }

    /// Moves a call that is active on another of this account's devices to this one.
    pub fn switch_here(&self, ringid: String) {
        if self.has_active_call() {
            self.toast("Hang up here first.");
            return;
        }
        let client = self.client();
        let app = self.clone();
        glib::spawn_future_local(async move {
            match runtime::run(async move { client.media(&ringid).await }).await {
                Ok(answer) => app.begin_call(answer.call, answer.media),
                Err(err) => app.error(&err),
            }
        });
    }

    fn handoff(&self, ringid: &str, device_id: &str) {
        if self.active_ringid().as_deref() != Some(ringid) || device_id == self.my_device_id() {
            return;
        }
        let device_name = self
            .0
            .active
            .borrow()
            .as_ref()
            .and_then(|a| {
                a.call.participants.iter().find_map(|p| {
                    p.device
                        .as_ref()
                        .filter(|d| d.device_id == device_id)
                        .cloned()
                })
            })
            .map(|d| {
                d.name
                    .unwrap_or_else(|| widgets::platform_name(d.platform).to_string())
            })
            .unwrap_or_else(|| "another device".to_string());
        self.end_local_call(None, false);
        self.window().call.show_moved(&device_name);
        self.toast(&format!("Moved to {device_name}"));
        // The server's call.updated for the new device fills in the "switch back" banner.
        let client = self.client();
        let app = self.clone();
        let id = ringid.to_string();
        glib::spawn_future_local(async move {
            if let Ok(call) = runtime::run(async move { client.call(&id).await }).await {
                app.call_changed(call);
            }
        });
    }

    pub fn show_call(&self) {
        if self.has_active_call() {
            self.window().show(Page::Call);
            self.present();
        }
    }

    pub fn show_home(&self) {
        self.window().show(Page::Home);
    }

    // ---- media ------------------------------------------------------------------------------------

    fn connect_media(&self, grant: MediaGrant) {
        let generation = self.0.media_generation.get() + 1;
        self.0.media_generation.set(generation);
        self.drop_media_only();
        let config = self.config();
        // An empty id cannot name a device (see `call::selectable`); leave it to the default.
        let options = MediaOptions {
            input_device: config.input_device.filter(|id| !id.is_empty()),
            output_device: config.output_device.filter(|id| !id.is_empty()),
            muted: false,
        };
        self.window().call.set_media_state(&MediaState::Connecting);
        let app = self.clone();
        glib::spawn_future_local(async move {
            let result =
                runtime::run(async move { CallMedia::connect(&grant, options).await }).await;
            match result {
                Ok((media, mut events)) => {
                    if app.0.media_generation.get() != generation || !app.has_active_call() {
                        runtime::spawn(async move { media.disconnect().await });
                        return;
                    }
                    let muted = app.0.active.borrow().as_ref().is_some_and(|a| a.muted);
                    if muted {
                        let _guard = runtime::enter();
                        media.set_muted(true);
                    }
                    app.0.media.replace(Some(media));
                    app.window().call.set_media_state(&MediaState::Connected);
                    while let Some(event) = events.recv().await {
                        if app.0.media_generation.get() != generation {
                            break;
                        }
                        app.on_media_event(event);
                    }
                }
                Err(err) => {
                    if app.0.media_generation.get() == generation {
                        tracing::warn!("call media failed: {err}");
                        app.toast(&format!("Couldn't connect call audio: {err}"));
                        app.window()
                            .call
                            .set_media_state(&MediaState::Disconnected {
                                reason: err.to_string(),
                            });
                    }
                }
            }
        });
    }

    fn on_media_event(&self, event: MediaEvent) {
        let view = self.window().call;
        match event {
            MediaEvent::State(state) => {
                view.set_media_state(&state);
                match state {
                    MediaState::Connected => {
                        if let Some(active) = self.0.active.borrow_mut().as_mut() {
                            active.rejoins = 0;
                        }
                    }
                    MediaState::Disconnected { reason } => {
                        tracing::info!("call media disconnected: {reason}");
                        self.rejoin_media();
                    }
                    MediaState::Connecting | MediaState::Reconnecting => {}
                }
            }
            MediaEvent::ActiveSpeakers { identities } => view.set_speaking(&identities),
            MediaEvent::LocalLevel { level } => view.set_level(level),
            MediaEvent::RemoteMuted { identity, muted } => view.set_remote_muted(&identity, muted),
            MediaEvent::ParticipantJoined { .. } | MediaEvent::ParticipantLeft { .. } => {}
        }
    }

    /// After an unexpected media drop, asks for a fresh grant within the server's 30 s grace.
    ///
    /// LiveKit also drops this device when the call moves to another one, possibly before
    /// `call.handoff` arrives, so the call is checked first: asking for media blindly would pull
    /// the call straight back.
    fn rejoin_media(&self) {
        let (ringid, attempt) = {
            let mut active = self.0.active.borrow_mut();
            match active.as_mut() {
                Some(a) if !a.rejoining && a.call.state != CallState::Ended => {
                    a.rejoining = true;
                    a.rejoins += 1;
                    (a.call.ringid.clone(), a.rejoins)
                }
                _ => return,
            }
        };
        self.drop_media_only();
        let client = self.client();
        let my_device = self.my_device_id();
        let me = self.my_uuid();
        let app = self.clone();
        glib::spawn_future_local(async move {
            let delay = 1500u64 << attempt.saturating_sub(1).min(4);
            glib::timeout_future(Duration::from_millis(delay)).await;
            let id = ringid.clone();
            let result = runtime::run(async move {
                let call = client.call(&id).await?;
                let mine = call
                    .participants
                    .iter()
                    .find(|p| p.account.uuid == me && p.state == ParticipantState::Joined);
                let elsewhere = mine
                    .and_then(|p| p.device.as_ref())
                    .filter(|d| d.device_id != my_device)
                    .map(|d| d.device_id.clone());
                if call.state == CallState::Ended || mine.is_none() {
                    return Ok(Rejoin::Over(call));
                }
                if let Some(device_id) = elsewhere {
                    return Ok(Rejoin::MovedTo(call, device_id));
                }
                client.media(&id).await.map(Rejoin::Media)
            })
            .await;
            if let Some(active) = app.0.active.borrow_mut().as_mut() {
                active.rejoining = false;
            }
            if app.active_ringid().as_deref() != Some(ringid.as_str()) {
                return;
            }
            match result {
                Ok(Rejoin::Media(answer)) => {
                    if let Some(grant) = answer.media {
                        if let Some(active) = app.0.active.borrow_mut().as_mut() {
                            active.identity = Some(grant.identity.clone());
                        }
                        app.window().call.set_local_identity(&grant.identity);
                        app.connect_media(grant);
                    }
                    app.call_changed(answer.call);
                }
                Ok(Rejoin::MovedTo(call, device_id)) => {
                    // The fresh call names the device it moved to.
                    if let Some(active) = app.0.active.borrow_mut().as_mut() {
                        active.call = call;
                    }
                    app.handoff(&ringid, &device_id);
                }
                Ok(Rejoin::Over(call)) => app.end_local_call(Some(call), true),
                Err(err) => {
                    if matches!(err.code(), codes::CALL_ENDED | codes::NOT_PARTICIPANT) {
                        app.end_local_call(None, true);
                    } else {
                        // Still inside the grace period, most likely; the next drop retries.
                        tracing::warn!("rejoining the call audio failed: {err}");
                        app.window()
                            .call
                            .set_media_state(&MediaState::Disconnected {
                                reason: err.to_string(),
                            });
                        app.rejoin_after_failure();
                    }
                }
            }
        });
    }

    /// Schedules another rejoin when asking for media failed (offline, server busy).
    fn rejoin_after_failure(&self) {
        let attempts = self.0.active.borrow().as_ref().map(|a| a.rejoins);
        match attempts {
            Some(n) if n < 6 => self.rejoin_media(),
            Some(_) => {
                self.toast("Lost the call audio. Hang up and call again.");
            }
            None => {}
        }
    }

    fn drop_media_only(&self) {
        if let Some(media) = self.0.media.take() {
            runtime::spawn(async move { media.disconnect().await });
        }
    }

    fn drop_media(&self) {
        self.0
            .media_generation
            .set(self.0.media_generation.get() + 1);
        self.drop_media_only();
    }

    pub fn set_muted(&self, muted: bool) {
        if let Some(active) = self.0.active.borrow_mut().as_mut() {
            active.muted = muted;
        }
        if let Some(media) = self.0.media.borrow().as_ref() {
            let _guard = runtime::enter();
            media.set_muted(muted);
        }
        self.window().call.set_muted(muted);
    }

    /// Chooses the microphone (`None` follows the system default) and switches a live call to it.
    pub fn set_input_device(&self, id: Option<String>) {
        self.update_config(|c| c.input_device = id.clone());
        if self.0.media.borrow().is_none() {
            return;
        }
        let _guard = runtime::enter();
        let target = id.or_else(|| default_device(ring_core::media::input_devices()));
        if let (Some(media), Some(id)) = (self.0.media.borrow().as_ref(), target) {
            if let Err(err) = media.set_input_device(&id) {
                self.error(&err);
            }
        }
    }

    /// Chooses the speaker (`None` follows the system default) and switches a live call to it.
    pub fn set_output_device(&self, id: Option<String>) {
        self.update_config(|c| c.output_device = id.clone());
        if self.0.media.borrow().is_none() {
            return;
        }
        let _guard = runtime::enter();
        let target = id.or_else(|| default_device(ring_core::media::output_devices()));
        if let (Some(media), Some(id)) = (self.0.media.borrow().as_ref(), target) {
            if let Err(err) = media.set_output_device(&id) {
                self.error(&err);
            }
        }
    }

    // ---- voicemail --------------------------------------------------------------------------------

    fn notify_voicemail(&self, voicemail: &Voicemail) {
        let body = voicemail
            .transcript
            .clone()
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| voicemail.from.id.clone());
        self.0.notifier.show(
            &self.0.gtk,
            Note {
                id: format!("voicemail-{}", voicemail.voicemail_id),
                title: format!("Voicemail from {}", voicemail.from.label()),
                body,
                kind: NoteKind::Voicemail,
                target: voicemail.voicemail_id.clone(),
            },
        );
    }

    pub fn mark_voicemail_read(&self, voicemail_id: &str) {
        let client = self.client();
        let app = self.clone();
        let id = voicemail_id.to_string();
        self.0
            .notifier
            .withdraw(&self.0.gtk, &format!("voicemail-{voicemail_id}"));
        glib::spawn_future_local(async move {
            if let Ok(voicemail) =
                runtime::run(async move { client.mark_voicemail(&id, true).await }).await
            {
                app.window().home.voicemail.upsert(voicemail);
                app.update_unread();
            }
        });
    }

    pub fn delete_voicemail(&self, voicemail_id: &str) {
        let client = self.client();
        let app = self.clone();
        let id = voicemail_id.to_string();
        glib::spawn_future_local(async move {
            let target = id.clone();
            match runtime::run(async move { client.delete_voicemail(&target).await }).await {
                Ok(()) => {
                    app.window().home.voicemail.remove(&id);
                    app.update_unread();
                    app.toast("Voicemail deleted");
                }
                Err(err) => app.error(&err),
            }
        });
    }

    // ---- notification and menu actions ------------------------------------------------------------

    /// Routes a notification's default action (`<kind>:<target>`).
    pub fn show_target(&self, target: &str) {
        let (kind, id) = target.split_once(':').unwrap_or(("", target));
        match kind {
            "call" => {
                if self.0.incoming.borrow().contains_key(id) {
                    self.show_incoming(id);
                } else {
                    self.present();
                }
            }
            "voicemail" => {
                self.present();
                self.window().home.select_section("voicemail");
                crate::ui::voicemail::open_detail_by_id(id);
            }
            "missed" => {
                self.present();
                self.window().home.select_section("recents");
            }
            _ => self.present(),
        }
    }

    /// "Call back" from a missed-call notification: the target is the ringid.
    pub fn call_back(&self, ringid: &str) {
        let client = self.client();
        let app = self.clone();
        let id = ringid.to_string();
        glib::spawn_future_local(async move {
            match runtime::run(async move { client.call(&id).await }).await {
                Ok(call) => {
                    let me = app.my_uuid();
                    if let Some(person) = widgets::others(&call, &me).into_iter().next() {
                        app.start_call(person.id);
                    }
                }
                Err(err) => app.error(&err),
            }
        });
    }
}

/// Registers the application actions shared by menus, shortcuts and notifications.
///
/// Actions can arrive over D-Bus (notification buttons, the desktop entry's "New Call") at any
/// time, including when start-up failed and there is no controller; they are ignored then.
pub fn install_actions(gtk_app: &adw::Application) {
    let string = Some(glib::VariantTy::STRING);

    let action = gio::SimpleAction::new("answer-call", string);
    action.connect_activate(|_, param| {
        if let (Some(app), Some(id)) = (App::try_get(), param.and_then(|p| p.str())) {
            app.accept(id);
        }
    });
    gtk_app.add_action(&action);

    let action = gio::SimpleAction::new("decline-call", string);
    action.connect_activate(|_, param| {
        if let (Some(app), Some(id)) = (App::try_get(), param.and_then(|p| p.str())) {
            app.decline(id, None);
        }
    });
    gtk_app.add_action(&action);

    let action = gio::SimpleAction::new("show-target", string);
    action.connect_activate(|_, param| {
        if let (Some(app), Some(target)) = (App::try_get(), param.and_then(|p| p.str())) {
            app.show_target(target);
        }
    });
    gtk_app.add_action(&action);

    let action = gio::SimpleAction::new("call-back", string);
    action.connect_activate(|_, param| {
        if let (Some(app), Some(ringid)) = (App::try_get(), param.and_then(|p| p.str())) {
            app.present();
            app.call_back(ringid);
        }
    });
    gtk_app.add_action(&action);

    let action = gio::SimpleAction::new("new-call", None);
    action.connect_activate(|_, _| {
        if let Some(app) = App::try_get() {
            app.new_call();
        }
    });
    gtk_app.add_action(&action);

    let action = gio::SimpleAction::new("about", None);
    action.connect_activate(|_, _| {
        if App::try_get().is_some() {
            crate::ui::window::show_about();
        }
    });
    gtk_app.add_action(&action);

    let action = gio::SimpleAction::new("quit", None);
    let quitting = gtk_app.downgrade();
    action.connect_activate(move |_, _| match App::try_get() {
        Some(app) => app.quit(),
        None => {
            if let Some(gtk_app) = quitting.upgrade() {
                gtk_app.quit();
            }
        }
    });
    gtk_app.add_action(&action);

    gtk_app.set_accels_for_action("app.new-call", &["<Control>n"]);
    gtk_app.set_accels_for_action("app.quit", &["<Control>q"]);
    gtk_app.set_accels_for_action("window.close", &["<Control>w"]);
}
