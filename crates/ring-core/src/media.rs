//! Call audio through LiveKit (feature `media`): one microphone track published, every remote
//! audio track played. Uses LiveKit's `PlatformAudio` (WASAPI on Windows, PulseAudio/ALSA on
//! Linux, CoreAudio on macOS) with WebRTC echo cancellation, noise suppression and AGC. The
//! platform module captures at the device's own rate and WebRTC encodes mono Opus at 48 kHz;
//! the SDK has no capture-rate setting.
//!
//! With the platform audio device module active, WebRTC itself mixes every subscribed remote
//! audio track into the selected output device, so playout needs no code here: subscribing is
//! enough, and LiveKit auto-subscribes.
//!
//! On Apple targets the final binary must be linked with `-ObjC` (for example
//! `println!("cargo:rustc-link-arg=-ObjC")` in its build script). Without it the linker drops
//! libwebrtc's Objective-C categories and one of its static initializers aborts before `main`. A
//! library's build script cannot pass this on to the binaries that use it.

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use livekit::options::{AudioEncoding, TrackPublishOptions};
use livekit::prelude::*;
use livekit::webrtc::stats::RtcStats;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::proto::MediaGrant;
use crate::{Error, Result};

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
    ParticipantJoined {
        identity: String,
    },
    ParticipantLeft {
        identity: String,
    },
    /// Active speakers changed (identities, loudest first). Includes the local identity.
    ActiveSpeakers {
        identities: Vec<String>,
    },
    /// Local microphone level 0.0–1.0, ~10 Hz, for a level meter.
    LocalLevel {
        level: f32,
    },
    /// A remote participant muted/unmuted their mic.
    RemoteMuted {
        identity: String,
        muted: bool,
    },
}

/// How often the microphone level is sampled for [`MediaEvent::LocalLevel`].
const LEVEL_INTERVAL: Duration = Duration::from_millis(100);

/// Opus bitrate for the microphone: full-band mono speech, above LiveKit's 24 kbps `SPEECH`
/// preset. Unset, the SDK would use its 48 kbps music preset, which RED's redundant copy doubles
/// on the wire for little gain in speech.
const MICROPHONE_BITRATE: u64 = 32_000;

/// A connected call. Dropping it disconnects.
pub struct CallMedia {
    pub(crate) inner: Option<Inner>,
}

pub(crate) struct Inner {
    room: Arc<Room>,
    audio: PlatformAudio,
    track: LocalAudioTrack,
    muted: Arc<AtomicBool>,
    tasks: Vec<JoinHandle<()>>,
}

impl CallMedia {
    /// Connects to `grant.url` with `grant.token`, publishes the microphone and plays all remote
    /// audio. Returns once connected; events stream on the receiver until disconnect.
    pub async fn connect(
        grant: &MediaGrant,
        options: MediaOptions,
    ) -> Result<(CallMedia, mpsc::UnboundedReceiver<MediaEvent>)> {
        let audio = PlatformAudio::new()
            .map_err(|e| Error::Media(format!("could not open the audio devices: {e}")))?;
        if let Some(id) = &options.input_device {
            let device = find_input(&audio, id)?;
            audio
                .set_recording_device(&device)
                .map_err(|e| Error::Media(format!("could not select the microphone: {e}")))?;
        }
        if let Some(id) = &options.output_device {
            let device = find_output(&audio, id)?;
            audio
                .set_playout_device(&device)
                .map_err(|e| Error::Media(format!("could not select the speaker: {e}")))?;
        }

        let (tx, rx) = mpsc::unbounded_channel();
        let _ = tx.send(MediaEvent::State(MediaState::Connecting));

        let (room, room_events) = Room::connect(&grant.url, &grant.token, RoomOptions::default())
            .await
            .map_err(|e| Error::Media(format!("could not join the call audio: {e}")))?;
        let room = Arc::new(room);

        let track = LocalAudioTrack::create_audio_track("microphone", audio.rtc_source());
        // Muting before publishing announces the track as muted, so nothing leaks at the start.
        if options.muted {
            track.mute();
        }
        // RED carries each frame again in the next packet, so a lost packet is rebuilt rather
        // than concealed. DTX stays off: it stops sending in pauses and clips the first syllable
        // when speech resumes, which the transcript, the representative and the recording hear.
        let publish = TrackPublishOptions {
            source: TrackSource::Microphone,
            audio_encoding: Some(AudioEncoding {
                max_bitrate: MICROPHONE_BITRATE,
            }),
            dtx: false,
            red: true,
            ..Default::default()
        };
        if let Err(e) = room
            .local_participant()
            .publish_track(LocalTrack::Audio(track.clone()), publish)
            .await
        {
            let _ = room.close().await;
            return Err(Error::Media(format!(
                "could not publish the microphone: {e}"
            )));
        }

        let muted = Arc::new(AtomicBool::new(options.muted));
        let tasks = vec![
            tokio::spawn(forward_events(room_events, tx.clone())),
            tokio::spawn(report_local_level(track.clone(), muted.clone(), tx)),
        ];
        Ok((
            CallMedia {
                inner: Some(Inner {
                    room,
                    audio,
                    track,
                    muted,
                    tasks,
                }),
            },
            rx,
        ))
    }

    pub fn set_muted(&self, muted: bool) {
        let Some(inner) = &self.inner else { return };
        inner.muted.store(muted, Ordering::SeqCst);
        if muted {
            inner.track.mute();
        } else {
            inner.track.unmute();
        }
    }

    pub fn is_muted(&self) -> bool {
        self.inner
            .as_ref()
            .is_none_or(|inner| inner.track.is_muted())
    }

    pub fn set_input_device(&self, id: &str) -> Result<()> {
        let inner = self.inner.as_ref().ok_or_else(disconnected)?;
        let device = find_input(&inner.audio, id)?;
        inner
            .audio
            .switch_recording_device(&device)
            .map_err(|e| Error::Media(format!("could not switch the microphone: {e}")))
    }

    pub fn set_output_device(&self, id: &str) -> Result<()> {
        let inner = self.inner.as_ref().ok_or_else(disconnected)?;
        let device = find_output(&inner.audio, id)?;
        inner
            .audio
            .switch_playout_device(&device)
            .map_err(|e| Error::Media(format!("could not switch the speaker: {e}")))
    }

    /// Leaves the room and releases audio devices.
    pub async fn disconnect(mut self) {
        if let Some(inner) = self.inner.take() {
            for task in &inner.tasks {
                task.abort();
            }
            if let Err(e) = inner.room.close().await {
                tracing::debug!(error = %e, "closing the call audio failed");
            }
            drop(inner);
        }
    }
}

impl Drop for CallMedia {
    fn drop(&mut self) {
        let Some(inner) = self.inner.take() else {
            return;
        };
        for task in &inner.tasks {
            task.abort();
        }
        // A LiveKit room stays connected until closed, so close it in the background.
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                let _ = inner.room.close().await;
                drop(inner);
            });
        }
    }
}

/// Lists input (microphone) devices.
pub fn input_devices() -> Result<Vec<AudioDevice>> {
    let audio = PlatformAudio::new()
        .map_err(|e| Error::Media(format!("could not open the audio devices: {e}")))?;
    Ok(with_default(
        audio
            .recording_devices()
            .map(|d| (d.id.as_str().to_owned(), d.name))
            .collect(),
    ))
}

/// Lists output (speaker/headset) devices.
pub fn output_devices() -> Result<Vec<AudioDevice>> {
    let audio = PlatformAudio::new()
        .map_err(|e| Error::Media(format!("could not open the audio devices: {e}")))?;
    Ok(with_default(
        audio
            .playout_devices()
            .map(|d| (d.id.as_str().to_owned(), d.name))
            .collect(),
    ))
}

/// WebRTC lists the system default first and names it "default …" on macOS and Linux and
/// "Default …" on Windows; when no entry says so, the first one is what it opens by default.
fn with_default(devices: Vec<(String, String)>) -> Vec<AudioDevice> {
    let named = devices
        .iter()
        .position(|(_, name)| name.to_ascii_lowercase().starts_with("default"));
    let default_index = named.or(if devices.is_empty() { None } else { Some(0) });
    devices
        .into_iter()
        .enumerate()
        .map(|(index, (id, name))| AudioDevice {
            id,
            name,
            is_default: Some(index) == default_index,
        })
        .collect()
}

fn find_input(audio: &PlatformAudio, id: &str) -> Result<RecordingDeviceId> {
    audio
        .recording_devices()
        .find(|d| d.id.as_str() == id)
        .map(|d| d.id)
        .ok_or_else(|| Error::Media(format!("no microphone with id {id:?}")))
}

fn find_output(audio: &PlatformAudio, id: &str) -> Result<PlayoutDeviceId> {
    audio
        .playout_devices()
        .find(|d| d.id.as_str() == id)
        .map(|d| d.id)
        .ok_or_else(|| Error::Media(format!("no speaker with id {id:?}")))
}

fn disconnected() -> Error {
    Error::Media("the call audio is disconnected".into())
}

async fn forward_events(
    mut events: mpsc::UnboundedReceiver<RoomEvent>,
    tx: mpsc::UnboundedSender<MediaEvent>,
) {
    let mut present: HashSet<String> = HashSet::new();
    while let Some(event) = events.recv().await {
        let mut out = Vec::new();
        match event {
            RoomEvent::Connected {
                participants_with_tracks,
            } => {
                out.push(MediaEvent::State(MediaState::Connected));
                for (participant, publications) in participants_with_tracks {
                    let identity = participant.identity().to_string();
                    if present.insert(identity.clone()) {
                        out.push(MediaEvent::ParticipantJoined {
                            identity: identity.clone(),
                        });
                    }
                    if let Some(publication) =
                        publications.iter().find(|p| p.kind() == TrackKind::Audio)
                    {
                        out.push(MediaEvent::RemoteMuted {
                            identity,
                            muted: publication.is_muted(),
                        });
                    }
                }
            }
            RoomEvent::ParticipantConnected(participant) => {
                let identity = participant.identity().to_string();
                if present.insert(identity.clone()) {
                    out.push(MediaEvent::ParticipantJoined { identity });
                }
            }
            RoomEvent::ParticipantDisconnected(participant) => {
                let identity = participant.identity().to_string();
                if present.remove(&identity) {
                    out.push(MediaEvent::ParticipantLeft { identity });
                }
            }
            RoomEvent::TrackPublished {
                publication,
                participant,
            } if publication.kind() == TrackKind::Audio => {
                out.push(MediaEvent::RemoteMuted {
                    identity: participant.identity().to_string(),
                    muted: publication.is_muted(),
                });
            }
            RoomEvent::ActiveSpeakersChanged { mut speakers } => {
                speakers.sort_by(|a, b| b.audio_level().total_cmp(&a.audio_level()));
                out.push(MediaEvent::ActiveSpeakers {
                    identities: speakers.iter().map(|p| p.identity().to_string()).collect(),
                });
            }
            RoomEvent::TrackMuted {
                participant: Participant::Remote(participant),
                publication,
            } if publication.kind() == TrackKind::Audio => {
                out.push(MediaEvent::RemoteMuted {
                    identity: participant.identity().to_string(),
                    muted: true,
                });
            }
            RoomEvent::TrackUnmuted {
                participant: Participant::Remote(participant),
                publication,
            } if publication.kind() == TrackKind::Audio => {
                out.push(MediaEvent::RemoteMuted {
                    identity: participant.identity().to_string(),
                    muted: false,
                });
            }
            RoomEvent::Reconnecting => out.push(MediaEvent::State(MediaState::Reconnecting)),
            RoomEvent::Reconnected => out.push(MediaEvent::State(MediaState::Connected)),
            RoomEvent::Disconnected { reason } => {
                let _ = tx.send(MediaEvent::State(MediaState::Disconnected {
                    reason: format!("{reason:?}"),
                }));
                return;
            }
            _ => {}
        }
        for event in out {
            if tx.send(event).is_err() {
                return;
            }
        }
    }
    let _ = tx.send(MediaEvent::State(MediaState::Disconnected {
        reason: "closed".into(),
    }));
}

/// Samples the microphone level from WebRTC's media-source stats, which the platform capture path
/// fills in. Only changes are reported, so a silent or muted mic costs nothing downstream.
async fn report_local_level(
    track: LocalAudioTrack,
    muted: Arc<AtomicBool>,
    tx: mpsc::UnboundedSender<MediaEvent>,
) {
    let mut tick = tokio::time::interval(LEVEL_INTERVAL);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut last = 0.0f32;
    loop {
        tick.tick().await;
        let level = if muted.load(Ordering::SeqCst) {
            0.0
        } else {
            match track.get_stats().await {
                Ok(stats) => stats
                    .iter()
                    .find_map(|s| match s {
                        RtcStats::MediaSource(m) if m.source.kind == "audio" => {
                            Some(m.audio.audio_level as f32)
                        }
                        _ => None,
                    })
                    .unwrap_or(0.0)
                    .clamp(0.0, 1.0),
                Err(_) => continue,
            }
        };
        if (level - last).abs() < 0.005 {
            continue;
        }
        last = level;
        if tx.send(MediaEvent::LocalLevel { level }).is_err() {
            return;
        }
    }
}
