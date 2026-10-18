//! LiveKit room plumbing shared by every server-side participant.

use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{Context, Result};
use livekit::{
    options::TrackPublishOptions,
    prelude::*,
    webrtc::{
        audio_frame::AudioFrame,
        audio_source::{AudioSourceOptions, RtcAudioSource, native::NativeAudioSource},
    },
};
use tokio::{sync::Notify, sync::mpsc::UnboundedReceiver, task::JoinHandle};

use crate::{audio::Feeds, preroll};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);

/// How a participant's connection is set up.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Joining {
    /// Publishes as well as subscribes.
    Speaker,
    /// Only subscribes (the recorder). On LiveKit's single peer connection every new
    /// subscription waits for the participant to renegotiate, which the SDK debounces by 150 ms
    /// unless it may publish, so the first 150 ms of each new track would never arrive. With a
    /// separate subscriber connection the server offers each new track itself, within 10 ms.
    Listener,
}

pub async fn join(
    url: &str,
    token: &str,
    joining: Joining,
) -> Result<(Room, UnboundedReceiver<RoomEvent>)> {
    let mut options = RoomOptions::default();
    options.auto_subscribe = true;
    options.single_peer_connection = joining == Joining::Speaker;
    let (room, events) = tokio::time::timeout(CONNECT_TIMEOUT, Room::connect(url, token, options))
        .await
        .context("timed out joining the LiveKit room")?
        .context("could not join the LiveKit room")?;
    tracing::debug!(room = %room.name(), identity = %room.local_participant().identity(), "joined room");
    Ok((room, events))
}

/// What a session needs to know after the shared bookkeeping of a room event.
pub enum Change {
    None,
    /// A participant left the room.
    Left(String),
    /// The call's room was ended by the server (deleted, closed, or this participant removed).
    Ended,
    /// The connection to the room failed for good.
    Disconnected(String),
}

/// Keeps `feeds` in step with the room's audio subscriptions.
pub fn apply(feeds: &mut Feeds, event: RoomEvent) -> Change {
    match event {
        RoomEvent::TrackSubscribed {
            track: RemoteTrack::Audio(track),
            publication,
            participant,
        } => {
            let announced = publication
                .audio_features()
                .contains(&AudioTrackFeature::TfPreconnectBuffer);
            feeds.add(
                publication.sid().to_string(),
                participant.identity().to_string(),
                track,
                announced,
            );
            Change::None
        }
        RoomEvent::ByteStreamOpened {
            reader,
            topic,
            participant_identity,
        } if topic == preroll::TOPIC => {
            if let Some(reader) = reader.take() {
                feeds.receive_preroll(participant_identity.as_str(), reader);
            }
            Change::None
        }
        RoomEvent::TrackUnsubscribed { publication, .. } => {
            feeds.remove(publication.sid().as_str());
            Change::None
        }
        RoomEvent::ParticipantDisconnected(participant) => {
            let identity = participant.identity().to_string();
            feeds.remove_identity(&identity);
            Change::Left(identity)
        }
        RoomEvent::Disconnected { reason } => match reason {
            livekit::DisconnectReason::RoomDeleted
            | livekit::DisconnectReason::RoomClosed
            | livekit::DisconnectReason::ParticipantRemoved
            | livekit::DisconnectReason::ClientInitiated => Change::Ended,
            reason => Change::Disconnected(format!("{reason:?}")),
        },
        RoomEvent::Reconnecting => {
            tracing::info!("LiveKit connection interrupted; reconnecting");
            Change::None
        }
        RoomEvent::Reconnected => {
            tracing::info!("LiveKit connection restored");
            Change::None
        }
        _ => Change::None,
    }
}

/// Samples per 10 ms frame, which is what libwebrtc consumes.
fn frame_len(rate: u32) -> usize {
    (rate / 100) as usize
}

struct Queue {
    samples: Mutex<VecDeque<i16>>,
    pushed: Notify,
}

/// A published microphone track fed from PCM pushed by the session. A pump task hands it to
/// libwebrtc in 10 ms frames; `capture_frame` blocks while the source's own short queue is
/// full, which paces playback in real time. Audio arrives in bursts (GPT-Live sends about
/// 100 ms at a time), so playback starts only once `PRIME` is queued; otherwise every slightly
/// late burst would be heard as a gap.
pub struct Voice {
    queue: Arc<Queue>,
    pump: JoinHandle<()>,
    max_backlog: usize,
    _track: LocalAudioTrack,
}

/// libwebrtc-side buffering; kept short so interruptions are heard quickly.
const SOURCE_QUEUE_MS: u32 = 50;
/// Jitter allowance before (re)starting playback.
const PRIME: Duration = Duration::from_millis(120);
/// A queue this quiet means the stream ended; play out whatever is left.
const TAIL_FLUSH: Duration = Duration::from_millis(150);

impl Voice {
    pub async fn publish(room: &Room, rate: u32, max_backlog: Duration) -> Result<Self> {
        let source = NativeAudioSource::new(
            AudioSourceOptions {
                echo_cancellation: false,
                noise_suppression: false,
                auto_gain_control: false,
            },
            rate,
            1,
            SOURCE_QUEUE_MS,
        );
        let track =
            LocalAudioTrack::create_audio_track("voice", RtcAudioSource::Native(source.clone()));
        room.local_participant()
            .publish_track(
                LocalTrack::Audio(track.clone()),
                TrackPublishOptions {
                    source: TrackSource::Microphone,
                    dtx: false,
                    ..Default::default()
                },
            )
            .await
            .context("could not publish the audio track")?;
        let queue = Arc::new(Queue {
            samples: Mutex::new(VecDeque::new()),
            pushed: Notify::new(),
        });
        let pump = tokio::spawn(pump(source.clone(), queue.clone(), rate));
        Ok(Self {
            queue,
            pump,
            max_backlog: (rate as u128 * max_backlog.as_millis() / 1000) as usize,
            _track: track,
        })
    }

    pub fn push(&self, samples: &[i16]) {
        let mut queue = self.queue.samples.lock().unwrap();
        queue.extend(samples);
        let excess = queue.len().saturating_sub(self.max_backlog);
        if excess > 0 {
            queue.drain(..excess);
            tracing::debug!(dropped_samples = excess, "voice backlog trimmed");
        }
        drop(queue);
        self.queue.pushed.notify_one();
    }

    /// Queues a prepared clip in full, past the backlog limit, so none of it is cut.
    pub fn push_clip(&self, samples: &[i16]) {
        self.queue.samples.lock().unwrap().extend(samples);
        self.queue.pushed.notify_one();
    }

    pub fn queued(&self) -> usize {
        self.queue.samples.lock().unwrap().len()
    }

    /// Plays `samples` and returns once they have been handed to the network.
    pub async fn play(&self, samples: &[i16]) {
        self.push(samples);
        while self.queued() > 0 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        tokio::time::sleep(Duration::from_millis(SOURCE_QUEUE_MS as u64 + 40)).await;
    }
}

impl Drop for Voice {
    fn drop(&mut self) {
        self.pump.abort();
    }
}

async fn pump(source: NativeAudioSource, queue: Arc<Queue>, rate: u32) {
    let n = frame_len(rate);
    let prime = (rate as u128 * PRIME.as_millis() / 1000) as usize;
    let mut playing = false;
    // Set once the stream went quiet: whatever is still queued plays out without re-priming.
    let mut draining = false;
    loop {
        let frame = {
            let mut samples = queue.samples.lock().unwrap();
            if samples.len() >= prime {
                playing = true;
            }
            if samples.is_empty() {
                draining = false;
            }
            if playing && !draining && samples.len() < n {
                playing = false;
                if !samples.is_empty() {
                    tracing::trace!("voice underrun");
                }
            }
            ((playing || draining) && !samples.is_empty()).then(|| {
                let take = n.min(samples.len());
                let mut frame: Vec<i16> = samples.drain(..take).collect();
                frame.resize(n, 0);
                frame
            })
        };
        let Some(frame) = frame else {
            if tokio::time::timeout(TAIL_FLUSH, queue.pushed.notified())
                .await
                .is_err()
            {
                // Nothing new arrived: the stream ended, so play out what is left.
                draining = true;
            }
            continue;
        };
        let frame = AudioFrame {
            data: frame.into(),
            sample_rate: rate,
            num_channels: 1,
            samples_per_channel: n as u32,
        };
        if let Err(e) = source.capture_frame(&frame).await {
            tracing::warn!("audio capture failed: {}", e.message);
        }
    }
}
