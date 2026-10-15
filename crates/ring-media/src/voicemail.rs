//! Voicemail: the player (`ring:voicemail`) that greets a carbon caller, beeps and records them,
//! and the synthesis of a silicon's text message in its own voice.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result, bail};
use ring_proto::internal::FromMedia;
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::{
    audio::{self, Buffering, Clock, Feeds, LIVE_RATE, RECORD_RATE, WavWriter},
    live,
    recorder::file_name,
    room::{self, Change, Voice},
    stt,
    supervisor::Supervisor,
};

pub struct Params {
    pub ringid: String,
    pub voicemail_id: String,
    pub caller_identity: String,
    pub greeting_text: String,
    pub voice: String,
    pub max_seconds: u64,
    pub token: String,
    pub s3_key: String,
}

/// How long to wait for the caller's audio before greeting anyway, and how long the caller may
/// be missing from the room before the voicemail ends.
const CALLER_WAIT: Duration = Duration::from_secs(5);

pub async fn run(supervisor: Arc<Supervisor>, params: Params, stop: CancellationToken) {
    tracing::info!(ringid = %params.ringid, voicemail_id = %params.voicemail_id, "voicemail starting");
    let finished = match record(&supervisor, &params, &stop).await {
        // The caller has not been recorded yet: report nothing, so ring-server starts this
        // voicemail again (greeting first) on the next worker instead of dropping it.
        Err(e) if e.is::<Restarting>() => {
            tracing::info!(voicemail_id = %params.voicemail_id, "voicemail left for a worker restart");
            return;
        }
        Ok((path, duration_ms)) => {
            deliver(
                &supervisor,
                &params.voicemail_id,
                &path,
                &params.s3_key,
                duration_ms,
            )
            .await
        }
        Err(e) => {
            tracing::info!(voicemail_id = %params.voicemail_id, "no voicemail recorded: {e:#}");
            FromMedia::VoicemailFinished {
                voicemail_id: params.voicemail_id.clone(),
                s3_key: None,
                duration_ms: None,
                transcript: None,
                error: Some(format!("{e:#}")),
            }
        }
    };
    supervisor.reporter.send(finished);
}

/// Uploads the recorded message and transcribes it. A failed transcription is reported as an
/// error next to the audio; a transcript is never made up.
async fn deliver(
    supervisor: &Supervisor,
    voicemail_id: &str,
    path: &Path,
    s3_key: &str,
    duration_ms: u64,
) -> FromMedia {
    let failed = |error: String| FromMedia::VoicemailFinished {
        voicemail_id: voicemail_id.to_owned(),
        s3_key: None,
        duration_ms: Some(duration_ms),
        transcript: None,
        error: Some(error),
    };
    if let Err(e) = supervisor.services.upload_wav(path, s3_key).await {
        tracing::warn!(%voicemail_id, path = %path.display(), "voicemail kept locally: {e:#}");
        return failed(format!("{e:#}"));
    }
    let wav = std::fs::read(path);
    let _ = std::fs::remove_file(path);
    let transcription = match (wav, supervisor.config.deepgram_key()) {
        (Ok(wav), Ok(key)) => stt::transcribe(&supervisor.services.http, key, wav).await,
        (Err(e), _) => Err(e.into()),
        (_, Err(e)) => Err(e),
    };
    let (transcript, error) = match transcription {
        Ok(text) => (Some(text), None),
        Err(e) => {
            tracing::warn!(%voicemail_id, "voicemail transcription failed: {e:#}");
            (None, Some(format!("transcription failed: {e:#}")))
        }
    };
    tracing::info!(%voicemail_id, duration_ms, "voicemail delivered");
    FromMedia::VoicemailFinished {
        voicemail_id: voicemail_id.to_owned(),
        s3_key: Some(s3_key.to_owned()),
        duration_ms: Some(duration_ms),
        transcript,
        error,
    }
}

async fn record(
    supervisor: &Supervisor,
    p: &Params,
    stop: &CancellationToken,
) -> Result<(PathBuf, u64)> {
    let greeting = async {
        tokio::select! {
            greeting = greeting(supervisor, &p.voice, &p.greeting_text) => Some(greeting),
            _ = stop.cancelled() => None,
        }
    };
    // The join itself is not cancelled: a half-finished join would leave a participant behind.
    let (greeting, joined) = tokio::join!(
        greeting,
        room::join(&supervisor.config.livekit_url, &p.token)
    );
    let (room, events) = joined?;
    let Some(greeting) = greeting.filter(|_| !stop.is_cancelled()) else {
        let _ = room.close().await;
        if supervisor.shutdown.is_cancelled() {
            return Err(Restarting.into());
        }
        bail!("the caller hung up before the greeting");
    };
    let greeting = greeting.unwrap_or_else(|e| {
        tracing::warn!(voicemail_id = %p.voicemail_id, "greeting unavailable, beeping only: {e:#}");
        Vec::new()
    });
    let mut player = Player {
        room: &room,
        events,
        feeds: Feeds::new(RECORD_RATE, Buffering::RECORDING),
        caller: &p.caller_identity,
        stop,
    };
    let result = async {
        let voice = Voice::publish(&room, LIVE_RATE, Duration::from_secs(600)).await?;
        if let Err(e) = player.greet(&voice, greeting).await {
            return Err(if supervisor.shutdown.is_cancelled() {
                Restarting.into()
            } else {
                e
            });
        }
        let path = supervisor
            .config
            .data_dir
            .join("voicemails")
            .join(format!("{}.wav", file_name(&p.voicemail_id)));
        let max_seconds = match p.max_seconds {
            0 => ring_proto::VOICEMAIL_MAX_SECONDS,
            seconds => seconds,
        };
        player.record(&path, Duration::from_secs(max_seconds)).await
    }
    .await;
    let _ = room.close().await;
    result
}

/// The worker is shutting down before the caller was recorded.
#[derive(Debug)]
struct Restarting;

impl std::fmt::Display for Restarting {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ring-media is restarting")
    }
}

impl std::error::Error for Restarting {}

/// Why the voicemail cannot go on.
enum Interrupt {
    /// voicemail.stop, call.closed, shutdown, or the caller left the room.
    HungUp,
    RoomGone(String),
}

struct Player<'a> {
    room: &'a livekit::Room,
    events: mpsc::UnboundedReceiver<livekit::RoomEvent>,
    feeds: Feeds,
    caller: &'a str,
    stop: &'a CancellationToken,
}

impl Player<'_> {
    /// Whether the caller is in the room. A caller who never joined (or left before this player
    /// did) produces no leave event, so presence is checked as well.
    fn caller_present(&self) -> bool {
        self.room
            .remote_participants()
            .keys()
            .any(|identity| identity.as_str() == self.caller)
    }

    /// Handles one room event (or the stop signal); returns what ended the voicemail, if anything.
    async fn event(&mut self) -> Option<Interrupt> {
        let event = tokio::select! {
            _ = self.stop.cancelled() => return Some(Interrupt::HungUp),
            event = self.events.recv() => event,
        };
        match event.map(|e| room::apply(&mut self.feeds, e)) {
            Some(Change::Left(who)) if who == self.caller => Some(Interrupt::HungUp),
            Some(Change::Ended) => Some(Interrupt::HungUp),
            Some(Change::Disconnected(reason)) => Some(Interrupt::RoomGone(reason)),
            None => Some(Interrupt::RoomGone("closed".into())),
            _ => None,
        }
    }

    /// Plays the greeting and the beep, once the caller is there to hear them.
    async fn greet(&mut self, voice: &Voice, greeting: Vec<i16>) -> Result<()> {
        let deadline = tokio::time::Instant::now() + CALLER_WAIT;
        while !self.feeds.has_identity(self.caller) {
            match tokio::time::timeout_at(deadline, self.event()).await {
                Err(_) => break,
                Ok(Some(interrupt)) => return Err(interrupted(interrupt)),
                Ok(None) => {}
            }
        }
        if !self.caller_present() {
            bail!("the caller is not in the call");
        }
        let mut prompt = greeting;
        prompt.extend(audio::silence(LIVE_RATE, Duration::from_millis(350)));
        prompt.extend(audio::beep(LIVE_RATE));
        let playback = voice.play(&prompt);
        tokio::pin!(playback);
        loop {
            tokio::select! {
                _ = &mut playback => return Ok(()),
                interrupt = self.event() => if let Some(interrupt) = interrupt {
                    return Err(interrupted(interrupt));
                },
            }
        }
    }

    /// Records only the caller, from the end of the beep until they hang up or `max` passes.
    async fn record(&mut self, path: &Path, max: Duration) -> Result<(PathBuf, u64)> {
        self.feeds.clear();
        let max_samples = (max.as_millis() as usize) * RECORD_RATE as usize / 1000;
        let mut wav = WavWriter::create(path, RECORD_RATE)?;
        let mut written = 0usize;
        let mut clock = Clock::new(RECORD_RATE);
        let mut tick = audio::tick_interval();
        let mut presence = tokio::time::interval(Duration::from_secs(1));
        let mut missing_since: Option<tokio::time::Instant> = None;
        while written < max_samples {
            tokio::select! {
                _ = tick.tick() => {
                    let n = clock.due().min(max_samples - written);
                    let samples = self.feeds.pull(n).remove(self.caller);
                    wav.write(&samples.map_or_else(|| vec![0; n], |s| audio::clip(&s)))?;
                    written += n;
                }
                _ = presence.tick() => {
                    if self.caller_present() {
                        missing_since = None;
                    } else if missing_since.get_or_insert_with(tokio::time::Instant::now).elapsed() >= CALLER_WAIT {
                        break;
                    }
                }
                interrupt = self.event() => if interrupt.is_some() {
                    break;
                },
            }
        }
        // The jitter buffer still holds the caller's last words.
        let (_, mut tail) = self.feeds.drain();
        if let Some(samples) = tail.remove(self.caller) {
            let n = samples.len().min(max_samples - written);
            wav.write(&audio::clip(&samples[..n]))?;
        }
        if wav.duration_ms() == 0 {
            let (path, _) = wav.finish()?;
            let _ = std::fs::remove_file(path);
            bail!("the caller hung up at the beep");
        }
        wav.finish()
    }
}

fn interrupted(interrupt: Interrupt) -> anyhow::Error {
    match interrupt {
        Interrupt::HungUp => anyhow::anyhow!("the caller hung up before the beep"),
        Interrupt::RoomGone(reason) => anyhow::anyhow!("disconnected from the room ({reason})"),
    }
}

/// A greeting in `voice`, cached by voice and text.
async fn greeting(supervisor: &Supervisor, voice: &str, text: &str) -> Result<Vec<i16>> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(Vec::new());
    }
    let digest = Sha256::digest(format!("{voice}|{text}").as_bytes());
    let name: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    let path = supervisor
        .config
        .data_dir
        .join("greetings")
        .join(format!("{name}.wav"));
    if let Ok(bytes) = std::fs::read(&path) {
        match audio::parse_wav(&bytes) {
            Ok((LIVE_RATE, samples)) if !samples.is_empty() => {
                tracing::debug!(%voice, "greeting from the cache");
                return Ok(samples);
            }
            _ => tracing::warn!(path = %path.display(), "ignoring an unreadable cached greeting"),
        }
    }
    let samples = live::synthesize(supervisor.config.openai_key()?, voice, text).await?;
    tracing::info!(
        %voice,
        duration_ms = samples.len() as u64 * 1000 / LIVE_RATE as u64,
        "greeting synthesized"
    );
    store(&path, &audio::wav_bytes(&samples, LIVE_RATE)).context("could not cache the greeting")?;
    Ok(samples)
}

fn store(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let partial = path.with_extension("partial");
    std::fs::write(&partial, bytes)?;
    std::fs::rename(&partial, path)?;
    Ok(())
}

/// A silicon's text voicemail, spoken in its own voice and uploaded.
pub async fn synthesize(
    supervisor: Arc<Supervisor>,
    voicemail_id: String,
    text: String,
    voice: String,
    s3_key: String,
) {
    tracing::info!(%voicemail_id, "synthesizing a voicemail");
    let result = async {
        let samples = live::synthesize(supervisor.config.openai_key()?, &voice, &text).await?;
        let path = supervisor
            .config
            .data_dir
            .join("voicemails")
            .join(format!("{}.wav", file_name(&voicemail_id)));
        store(&path, &audio::wav_bytes(&samples, LIVE_RATE))?;
        let uploaded = supervisor.services.upload_wav(&path, &s3_key).await;
        if uploaded.is_ok() {
            let _ = std::fs::remove_file(&path);
        }
        uploaded.map(|()| samples.len() as u64 * 1000 / LIVE_RATE as u64)
    }
    .await;
    let finished = match result {
        Ok(duration_ms) => FromMedia::VoicemailFinished {
            voicemail_id,
            s3_key: Some(s3_key),
            duration_ms: Some(duration_ms),
            transcript: Some(text),
            error: None,
        },
        Err(e) => {
            tracing::warn!(%voicemail_id, "voicemail synthesis failed: {e:#}");
            FromMedia::VoicemailFinished {
                voicemail_id,
                s3_key: None,
                duration_ms: None,
                transcript: None,
                error: Some(format!("{e:#}")),
            }
        }
    };
    supervisor.reporter.send(finished);
}
