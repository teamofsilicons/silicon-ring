//! C ABI over `ring-core` call media, built as `ring_ffi.dll` for the native Windows app.
//!
//! The contract is `windows/native/ring_ffi.h`. In short:
//!
//! - [`ring_media_connect`] returns a handle immediately and joins the LiveKit room in the
//!   background. Progress and everything else that happens on the call arrive as UTF-8 JSON
//!   events on one dedicated background thread (see [`event_json`] for the shapes).
//! - [`ring_media_disconnect`] leaves the room and frees the handle. Once it returns, the
//!   callback is never invoked again for that handle, so the caller may free `ctx`.
//! - Strings the library returns are owned by the caller and released with [`ring_string_free`].
//!
//! No panic crosses the boundary: every entry point runs under `catch_unwind` and reports a
//! failure through its return value and [`ring_ffi_last_error`].

use std::cell::RefCell;
use std::ffi::{CStr, CString, c_char, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, mpsc};
use std::thread::{self, ThreadId};
use std::time::Duration;

use ring_core::media::{self, AudioDevice, CallMedia, MediaEvent, MediaOptions, MediaState};
use ring_core::proto::{MediaGrant, Time};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::runtime::Runtime;
use tokio::task::JoinHandle;

/// Success.
pub const RING_OK: i32 = 0;
/// A required argument was null, not UTF-8, or the handle is no longer usable.
pub const RING_ERR_INVALID_ARGUMENT: i32 = -1;
/// The media layer reported an error; [`ring_ffi_last_error`] has the message.
pub const RING_ERR_MEDIA: i32 = -2;
/// A Rust panic was caught at the boundary; [`ring_ffi_last_error`] has the message.
pub const RING_ERR_INTERNAL: i32 = -3;

/// How long [`ring_media_disconnect`] waits for LiveKit to leave the room before giving up.
const DISCONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// `void callback(void *ctx, const char *event_json)`. The string is only valid during the call.
pub type RingMediaEventCallback =
    Option<unsafe extern "C" fn(ctx: *mut c_void, event_json: *const c_char)>;

// ---------------------------------------------------------------------------------------------
// Runtime and errors
// ---------------------------------------------------------------------------------------------

fn runtime() -> &'static Runtime {
    static RUNTIME: OnceLock<Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("ring-ffi")
            .enable_all()
            .build()
            .expect("failed to start the ring-ffi Tokio runtime")
    })
}

thread_local! {
    static LAST_ERROR: RefCell<Option<CString>> = const { RefCell::new(None) };
}

fn set_last_error(message: impl Into<String>) {
    let message = message.into().replace('\0', " ");
    LAST_ERROR.with(|slot| *slot.borrow_mut() = CString::new(message).ok());
}

fn clear_last_error() {
    LAST_ERROR.with(|slot| *slot.borrow_mut() = None);
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        format!("internal error: {s}")
    } else if let Some(s) = payload.downcast_ref::<String>() {
        format!("internal error: {s}")
    } else {
        "internal error".to_string()
    }
}

/// Runs `body`, turning a panic into `on_panic` and a recorded error message.
fn guard<T>(on_panic: T, body: impl FnOnce() -> T) -> T {
    match catch_unwind(AssertUnwindSafe(body)) {
        Ok(value) => value,
        Err(payload) => {
            set_last_error(panic_message(payload.as_ref()));
            on_panic
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Borrows a C string as UTF-8. `None` for null or invalid UTF-8.
///
/// # Safety
/// `ptr` must be null or point to a NUL-terminated string that outlives the returned borrow.
unsafe fn str_arg<'a>(ptr: *const c_char) -> Option<&'a str> {
    if ptr.is_null() {
        return None;
    }
    // SAFETY: guaranteed by the caller.
    unsafe { CStr::from_ptr(ptr) }.to_str().ok()
}

fn into_c_string(s: String) -> *mut c_char {
    CString::new(s.replace('\0', " "))
        .map(CString::into_raw)
        .unwrap_or(std::ptr::null_mut())
}

// ---------------------------------------------------------------------------------------------
// Event delivery
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct Target {
    callback: unsafe extern "C" fn(*mut c_void, *const c_char),
    // The caller's opaque pointer, carried as an integer so the sink is Send.
    ctx: usize,
}

/// Delivers events to the caller's callback from one dedicated thread, in order.
struct Sink {
    target: Mutex<Option<Target>>,
    closed: AtomicBool,
    thread: OnceLock<ThreadId>,
}

impl Sink {
    fn start(target: Target) -> (Arc<Sink>, mpsc::Sender<String>) {
        let sink = Arc::new(Sink {
            target: Mutex::new(Some(target)),
            closed: AtomicBool::new(false),
            thread: OnceLock::new(),
        });
        let (tx, rx) = mpsc::channel::<String>();
        let worker = Arc::clone(&sink);
        let spawned = thread::Builder::new()
            .name("ring-media-events".into())
            .spawn(move || {
                let _ = worker.thread.set(thread::current().id());
                while let Ok(json) = rx.recv() {
                    if worker.closed.load(Ordering::Acquire) {
                        break;
                    }
                    let Ok(text) = CString::new(json) else {
                        continue;
                    };
                    let target = lock(&worker.target);
                    if let Some(t) = *target {
                        // SAFETY: the caller promised the callback and ctx stay valid until
                        // ring_media_disconnect returns, and disconnect clears the target under this
                        // same lock before returning.
                        unsafe { (t.callback)(t.ctx as *mut c_void, text.as_ptr()) };
                    }
                }
            });
        if spawned.is_err() {
            sink.closed.store(true, Ordering::Release);
        }
        (sink, tx)
    }

    /// After this returns no callback is running or will run (unless called from inside the
    /// callback itself, in which case the current invocation is the last).
    fn silence(&self) {
        self.closed.store(true, Ordering::Release);
        if self.thread.get() == Some(&thread::current().id()) {
            return;
        }
        *lock(&self.target) = None;
    }
}

/// The JSON shape of every event delivered to the callback.
fn event_json(event: &MediaEvent) -> Option<String> {
    #[allow(unreachable_patterns)]
    let value = match event {
        MediaEvent::State(state) => state_json(state),
        MediaEvent::ParticipantJoined { identity } => {
            json!({ "type": "participant_joined", "identity": identity })
        }
        MediaEvent::ParticipantLeft { identity } => {
            json!({ "type": "participant_left", "identity": identity })
        }
        MediaEvent::ActiveSpeakers { identities } => {
            json!({ "type": "active_speakers", "identities": identities })
        }
        MediaEvent::LocalLevel { level } => {
            json!({ "type": "local_level", "level": level.clamp(0.0, 1.0) })
        }
        MediaEvent::RemoteMuted { identity, muted } => {
            json!({ "type": "remote_muted", "identity": identity, "muted": muted })
        }
        // Events added to ring-core later are not part of this ABI until they are mapped here.
        _ => return None,
    };
    Some(value.to_string())
}

fn state_json(state: &MediaState) -> Value {
    match state {
        MediaState::Connecting => json!({ "type": "state", "state": "connecting", "reason": null }),
        MediaState::Connected => json!({ "type": "state", "state": "connected", "reason": null }),
        MediaState::Reconnecting => {
            json!({ "type": "state", "state": "reconnecting", "reason": null })
        }
        MediaState::Disconnected { reason } => {
            json!({ "type": "state", "state": "disconnected", "reason": reason })
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Handle
// ---------------------------------------------------------------------------------------------

enum Slot {
    /// Joining the room. Holds the settings to apply once connected, so that mute and device
    /// choices made while connecting are not lost.
    Connecting(MediaOptions),
    Connected(CallMedia),
    /// Disconnected, failed, or being torn down.
    Closed {
        muted: bool,
    },
}

struct Shared {
    slot: Mutex<Slot>,
    events: Mutex<Option<mpsc::Sender<String>>>,
}

impl Shared {
    fn emit(&self, json: String) {
        if let Some(tx) = lock(&self.events).as_ref() {
            let _ = tx.send(json);
        }
    }
}

/// Opaque handle returned by [`ring_media_connect`].
pub struct RingMedia {
    shared: Arc<Shared>,
    sink: Arc<Sink>,
    task: JoinHandle<()>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct ConnectOptions {
    input_device: Option<String>,
    output_device: Option<String>,
    muted: bool,
    /// Informational; LiveKit takes the room and identity from the token.
    room: Option<String>,
    identity: Option<String>,
}

async fn run(shared: Arc<Shared>, grant: MediaGrant, options: MediaOptions) {
    shared.emit(state_json(&MediaState::Connecting).to_string());
    let (media, mut rx) = match CallMedia::connect(&grant, options.clone()).await {
        Ok(connected) => connected,
        Err(error) => {
            let reason = error.to_string();
            {
                let mut slot = lock(&shared.slot);
                let muted = match &*slot {
                    Slot::Connecting(o) => o.muted,
                    Slot::Closed { muted } => *muted,
                    Slot::Connected(m) => m.is_muted(),
                };
                *slot = Slot::Closed { muted };
            }
            shared.emit(state_json(&MediaState::Disconnected { reason }).to_string());
            return;
        }
    };

    let abandoned = {
        let mut slot = lock(&shared.slot);
        match std::mem::replace(
            &mut *slot,
            Slot::Closed {
                muted: options.muted,
            },
        ) {
            Slot::Connecting(wanted) => {
                apply_changes(&media, &options, &wanted);
                *slot = Slot::Connected(media);
                None
            }
            // ring_media_disconnect ran while we were connecting.
            other => {
                *slot = other;
                Some(media)
            }
        }
    };
    if let Some(media) = abandoned {
        let _ = tokio::time::timeout(DISCONNECT_TIMEOUT, media.disconnect()).await;
        return;
    }

    // ring-core queues its own Connecting before joining and usually a Connected once joined;
    // report each state once, in order.
    let mut last_state = Some(MediaState::Connected);
    shared.emit(state_json(&MediaState::Connected).to_string());
    while let Some(event) = rx.recv().await {
        if let MediaEvent::State(state) = &event {
            if *state == MediaState::Connecting || last_state.as_ref() == Some(state) {
                continue;
            }
            last_state = Some(state.clone());
        }
        if let Some(json) = event_json(&event) {
            shared.emit(json);
        }
    }
}

/// Applies mute and device choices that changed while the room was connecting.
fn apply_changes(media: &CallMedia, used: &MediaOptions, wanted: &MediaOptions) {
    if wanted.muted != used.muted {
        media.set_muted(wanted.muted);
    }
    if wanted.input_device != used.input_device {
        let id = wanted
            .input_device
            .clone()
            .or_else(|| default_device_id(media::input_devices()));
        if let Some(id) = id {
            let _ = media.set_input_device(&id);
        }
    }
    if wanted.output_device != used.output_device {
        let id = wanted
            .output_device
            .clone()
            .or_else(|| default_device_id(media::output_devices()));
        if let Some(id) = id {
            let _ = media.set_output_device(&id);
        }
    }
}

fn default_device_id(devices: ring_core::Result<Vec<AudioDevice>>) -> Option<String> {
    let devices = devices.ok()?;
    devices
        .iter()
        .find(|d| d.is_default)
        .or_else(|| devices.first())
        .map(|d| d.id.clone())
}

/// Shared implementation of the two device setters. `None` selects the system default.
fn set_device(handle: *mut RingMedia, id: *const c_char, input: bool) -> i32 {
    if handle.is_null() {
        set_last_error("handle is null");
        return RING_ERR_INVALID_ARGUMENT;
    }
    // SAFETY: a non-null handle came from ring_media_connect and has not been disconnected.
    let handle = unsafe { &*handle };
    let id = if id.is_null() {
        None
    } else {
        // SAFETY: the caller passes a NUL-terminated string valid for this call.
        match unsafe { str_arg(id) } {
            Some(s) if !s.is_empty() => Some(s.to_string()),
            Some(_) => None,
            None => {
                set_last_error("device id is not valid UTF-8");
                return RING_ERR_INVALID_ARGUMENT;
            }
        }
    };
    let _rt = runtime().enter();
    let mut slot = lock(&handle.shared.slot);
    match &mut *slot {
        Slot::Connecting(options) => {
            if input {
                options.input_device = id;
            } else {
                options.output_device = id;
            }
            RING_OK
        }
        Slot::Connected(media) => {
            let list = if input {
                media::input_devices()
            } else {
                media::output_devices()
            };
            let Some(id) = id.or_else(|| default_device_id(list)) else {
                set_last_error("no audio device is available");
                return RING_ERR_MEDIA;
            };
            let result = if input {
                media.set_input_device(&id)
            } else {
                media.set_output_device(&id)
            };
            match result {
                Ok(()) => RING_OK,
                Err(error) => {
                    set_last_error(error.to_string());
                    RING_ERR_MEDIA
                }
            }
        }
        Slot::Closed { .. } => {
            set_last_error("call media is disconnected");
            RING_ERR_INVALID_ARGUMENT
        }
    }
}

fn devices_json(list: ring_core::Result<Vec<AudioDevice>>) -> *mut c_char {
    match list {
        Ok(devices) => {
            let items: Vec<Value> = devices
                .iter()
                .map(|d| json!({ "id": d.id, "name": d.name, "is_default": d.is_default }))
                .collect();
            into_c_string(Value::Array(items).to_string())
        }
        Err(error) => {
            set_last_error(error.to_string());
            std::ptr::null_mut()
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Exported functions
// ---------------------------------------------------------------------------------------------

/// Library version, e.g. `"1.0.0"`. Static; do not free.
#[unsafe(no_mangle)]
pub extern "C" fn ring_ffi_version() -> *const c_char {
    concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr().cast()
}

/// The last error recorded on the calling thread, or null. Free with [`ring_string_free`].
#[unsafe(no_mangle)]
pub extern "C" fn ring_ffi_last_error() -> *mut c_char {
    guard(std::ptr::null_mut(), || {
        LAST_ERROR.with(|slot| {
            slot.borrow()
                .as_ref()
                .map(|s| s.clone().into_raw())
                .unwrap_or(std::ptr::null_mut())
        })
    })
}

/// Frees a string returned by this library. Null is ignored.
///
/// # Safety
/// `s` must be null or a pointer returned by this library that was not freed yet.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ring_string_free(s: *mut c_char) {
    if s.is_null() {
        return;
    }
    guard((), || {
        // SAFETY: the pointer came from CString::into_raw in this library.
        drop(unsafe { CString::from_raw(s) });
    });
}

/// Starts joining the call's LiveKit room and returns a handle at once, or null when an argument
/// is invalid (see [`ring_ffi_last_error`]). Connection progress and call events are delivered
/// to `callback` as JSON on a background thread.
///
/// `options_json` may be null or `{"input_device":id|null,"output_device":id|null,"muted":bool}`.
///
/// # Safety
/// `url`, `token` and `options_json` must be null or NUL-terminated strings valid for this call.
/// `callback` and `ctx` must stay valid until [`ring_media_disconnect`] returns for the handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ring_media_connect(
    url: *const c_char,
    token: *const c_char,
    options_json: *const c_char,
    callback: RingMediaEventCallback,
    ctx: *mut c_void,
) -> *mut RingMedia {
    guard(std::ptr::null_mut(), || {
        clear_last_error();
        // SAFETY: the caller passes NUL-terminated strings valid for this call.
        let (Some(url), Some(token)) = (unsafe { str_arg(url) }, unsafe { str_arg(token) }) else {
            set_last_error("url and token are required UTF-8 strings");
            return std::ptr::null_mut();
        };
        if url.is_empty() || token.is_empty() {
            set_last_error("url and token must not be empty");
            return std::ptr::null_mut();
        }
        let Some(callback) = callback else {
            set_last_error("callback is required");
            return std::ptr::null_mut();
        };
        let options: ConnectOptions = if options_json.is_null() {
            ConnectOptions::default()
        } else {
            // SAFETY: as above.
            let Some(text) = (unsafe { str_arg(options_json) }) else {
                set_last_error("options_json is not valid UTF-8");
                return std::ptr::null_mut();
            };
            if text.trim().is_empty() {
                ConnectOptions::default()
            } else {
                match serde_json::from_str(text) {
                    Ok(options) => options,
                    Err(error) => {
                        set_last_error(format!("options_json: {error}"));
                        return std::ptr::null_mut();
                    }
                }
            }
        };

        let media_options = MediaOptions {
            input_device: options.input_device.filter(|s| !s.is_empty()),
            output_device: options.output_device.filter(|s| !s.is_empty()),
            muted: options.muted,
        };
        let grant = MediaGrant {
            url: url.to_string(),
            token: token.to_string(),
            room: options.room.unwrap_or_default(),
            identity: options.identity.unwrap_or_default(),
            expires_at: Time::now(),
            preroll_to: None,
        };

        let (sink, events) = Sink::start(Target {
            callback,
            ctx: ctx as usize,
        });
        let shared = Arc::new(Shared {
            slot: Mutex::new(Slot::Connecting(media_options.clone())),
            events: Mutex::new(Some(events)),
        });
        let task = runtime().spawn(run(Arc::clone(&shared), grant, media_options));
        Box::into_raw(Box::new(RingMedia { shared, sink, task }))
    })
}

/// Mutes or unmutes the microphone. Works while connecting too.
///
/// # Safety
/// `handle` must be null or a live handle from [`ring_media_connect`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ring_media_set_muted(handle: *mut RingMedia, muted: i32) -> i32 {
    guard(RING_ERR_INTERNAL, || {
        if handle.is_null() {
            set_last_error("handle is null");
            return RING_ERR_INVALID_ARGUMENT;
        }
        // SAFETY: a non-null handle is live per the contract.
        let handle = unsafe { &*handle };
        let muted = muted != 0;
        let _rt = runtime().enter();
        let mut slot = lock(&handle.shared.slot);
        match &mut *slot {
            Slot::Connecting(options) => options.muted = muted,
            Slot::Connected(media) => media.set_muted(muted),
            Slot::Closed { muted: m } => *m = muted,
        }
        RING_OK
    })
}

/// 1 when the microphone is muted, 0 when it is live, a negative error code otherwise.
///
/// # Safety
/// `handle` must be null or a live handle from [`ring_media_connect`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ring_media_is_muted(handle: *mut RingMedia) -> i32 {
    guard(RING_ERR_INTERNAL, || {
        if handle.is_null() {
            set_last_error("handle is null");
            return RING_ERR_INVALID_ARGUMENT;
        }
        // SAFETY: a non-null handle is live per the contract.
        let handle = unsafe { &*handle };
        let _rt = runtime().enter();
        let muted = match &*lock(&handle.shared.slot) {
            Slot::Connecting(options) => options.muted,
            Slot::Connected(media) => media.is_muted(),
            Slot::Closed { muted } => *muted,
        };
        i32::from(muted)
    })
}

/// Switches the microphone. `device_id` null or empty selects the system default.
///
/// # Safety
/// `handle` must be null or a live handle; `device_id` null or a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ring_media_set_input_device(
    handle: *mut RingMedia,
    device_id: *const c_char,
) -> i32 {
    guard(RING_ERR_INTERNAL, || set_device(handle, device_id, true))
}

/// Switches the speaker/headset. `device_id` null or empty selects the system default.
///
/// # Safety
/// `handle` must be null or a live handle; `device_id` null or a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ring_media_set_output_device(
    handle: *mut RingMedia,
    device_id: *const c_char,
) -> i32 {
    guard(RING_ERR_INTERNAL, || set_device(handle, device_id, false))
}

/// Leaves the room, releases the audio devices and frees the handle. Blocks for at most a few
/// seconds; call it off the UI thread. After it returns the callback is never invoked again for
/// this handle. Null is ignored.
///
/// # Safety
/// `handle` must be null or a live handle from [`ring_media_connect`]; it is invalid afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ring_media_disconnect(handle: *mut RingMedia) {
    if handle.is_null() {
        return;
    }
    guard((), || {
        // SAFETY: the handle came from Box::into_raw in ring_media_connect and is freed only here.
        let handle = unsafe { Box::from_raw(handle) };
        handle.sink.silence();
        // Dropping the sender lets the event thread finish.
        lock(&handle.shared.events).take();

        let previous = {
            let mut slot = lock(&handle.shared.slot);
            let muted = match &*slot {
                Slot::Connecting(o) => o.muted,
                Slot::Connected(m) => m.is_muted(),
                Slot::Closed { muted } => *muted,
            };
            std::mem::replace(&mut *slot, Slot::Closed { muted })
        };
        if let Slot::Connected(media) = previous {
            handle.task.abort();
            let _ = runtime().block_on(async {
                tokio::time::timeout(DISCONNECT_TIMEOUT, media.disconnect()).await
            });
        }
        // While still connecting, `run` sees the Closed slot when the connect finishes and
        // disconnects on its own; aborting here would leak a half-joined room.
    });
}

/// The microphones as a JSON array `[{"id","name","is_default"}]`, or null on failure.
/// Free with [`ring_string_free`].
#[unsafe(no_mangle)]
pub extern "C" fn ring_media_input_devices_json() -> *mut c_char {
    guard(std::ptr::null_mut(), || {
        clear_last_error();
        let _rt = runtime().enter();
        devices_json(media::input_devices())
    })
}

/// The speakers/headsets as a JSON array `[{"id","name","is_default"}]`, or null on failure.
/// Free with [`ring_string_free`].
#[unsafe(no_mangle)]
pub extern "C" fn ring_media_output_devices_json() -> *mut c_char {
    guard(std::ptr::null_mut(), || {
        clear_last_error();
        let _rt = runtime().enter();
        devices_json(media::output_devices())
    })
}
