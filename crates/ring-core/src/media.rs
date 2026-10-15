//! Call audio through LiveKit (feature `media`): one microphone track published, every remote
//! audio track played. Uses LiveKit's `PlatformAudio` (WASAPI on Windows, PulseAudio/ALSA on
//! Linux, CoreAudio on macOS) with WebRTC echo cancellation, noise suppression and AGC.
//!
//! SKELETON: signatures are the contract for the Linux app and ring-ffi; the ring-core owner
//! implements them.

use tokio::sync::mpsc;

use crate::proto::MediaGrant;
use crate::Result;

/// An audio device as reported by the platform.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioDevice {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

#[derive(Clone, Debug, Default)]
pub struct MediaOptions {
    /// Input device id (None = system default).
    pub input_device: Option<String>,
    /// Output device id (None = system default).
    pub output_device: Option<String>,
    /// Start muted.
    pub muted: bool,
}

/// Connection state of the call media.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MediaState {
    Connecting,
    Connected,
    Reconnecting,
    Disconnected { reason: String },
}

#[derive(Clone, Debug, PartialEq)]
pub enum MediaEvent {
    State(MediaState),
    /// A remote participant joined the room. `identity` matches `Participant.media_identity`.
    ParticipantJoined { identity: String },
    ParticipantLeft { identity: String },
    /// Active speakers changed (identities, loudest first). Includes the local identity.
    ActiveSpeakers { identities: Vec<String> },
    /// Local microphone level 0.0–1.0, ~10 Hz, for a level meter.
    LocalLevel { level: f32 },
    /// A remote participant muted/unmuted their mic.
    RemoteMuted { identity: String, muted: bool },
}

/// A connected call. Dropping it disconnects.
pub struct CallMedia {
    pub(crate) _private: (),
}

impl CallMedia {
    /// Connects to `grant.url` with `grant.token`, publishes the microphone and plays all remote
    /// audio. Returns once connected; events stream on the receiver until disconnect.
    pub async fn connect(grant: &MediaGrant, options: MediaOptions) -> Result<(CallMedia, mpsc::UnboundedReceiver<MediaEvent>)> {
        let _ = (grant, options);
        todo!()
    }

    pub fn set_muted(&self, muted: bool) {
        let _ = muted;
        todo!()
    }

    pub fn is_muted(&self) -> bool {
        todo!()
    }

    pub fn set_input_device(&self, id: &str) -> Result<()> {
        let _ = id;
        todo!()
    }

    pub fn set_output_device(&self, id: &str) -> Result<()> {
        let _ = id;
        todo!()
    }

    /// Leaves the room and releases audio devices.
    pub async fn disconnect(self) {
        todo!()
    }
}

/// Lists input (microphone) devices.
pub fn input_devices() -> Result<Vec<AudioDevice>> {
    todo!()
}

/// Lists output (speaker/headset) devices.
pub fn output_devices() -> Result<Vec<AudioDevice>> {
    todo!()
}
