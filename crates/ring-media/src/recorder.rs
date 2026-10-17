//! The call recorder (`ring:recorder`): mixes every voice into one continuous 48 kHz WAV,
//! uploads it when the call ends, and streams each carbon's authoritative track to Deepgram.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use anyhow::Result;
use ring_proto::internal::{FromMedia, RosterMember};
use tokio::{sync::mpsc, task::JoinHandle};

use crate::{
    audio::{self, Buffering, Clock, Feeds, RECORD_RATE, WavWriter},
    room::{self, Change},
    roster::Roster,
    stt,
    supervisor::{RecControl, Session, Supervisor},
};

pub struct Params {
    pub ringid: String,
    pub token: String,
    pub roster: Vec<RosterMember>,
    pub s3_key: String,
}

/// About ten seconds of 20 ms chunks; Deepgram falling further behind than that drops audio.
const STT_QUEUE: usize = 500;

pub async fn run(supervisor: Arc<Supervisor>, params: Params, mut session: Session<RecControl>) {
    tracing::info!(ringid = %params.ringid, "recorder starting");
    let finished = match record(&supervisor, &params, &mut session).await {
        Ok((path, duration_ms)) if duration_ms > 0 => {
            match supervisor.services.upload_wav(&path, &params.s3_key).await {
                Ok(()) => {
                    let _ = std::fs::remove_file(&path);
                    tracing::info!(ringid = %params.ringid, duration_ms, "recording uploaded");
                    FromMedia::RecordingFinished {
                        ringid: params.ringid.clone(),
                        s3_key: Some(params.s3_key.clone()),
                        duration_ms: Some(duration_ms),
                        error: None,
                    }
                }
                Err(e) => {
                    tracing::warn!(ringid = %params.ringid, path = %path.display(), "recording kept locally: {e:#}");
                    failed(&params.ringid, format!("{e:#}"))
                }
            }
        }
        Ok((path, _)) => {
            let _ = std::fs::remove_file(&path);
            failed(&params.ringid, "no audio was recorded".into())
        }
        Err(e) => {
            tracing::warn!(ringid = %params.ringid, "recorder failed: {e:#}");
            failed(&params.ringid, format!("{e:#}"))
        }
    };
    supervisor.reporter.send(finished);
}

fn failed(ringid: &str, error: String) -> FromMedia {
    FromMedia::RecordingFinished {
        ringid: ringid.to_owned(),
        s3_key: None,
        duration_ms: None,
        error: Some(error),
    }
}

pub fn file_name(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Records until stopped or the room goes away. A recording interrupted by a media failure
/// is still finished and kept: a partial recording beats none.
async fn record(
    supervisor: &Supervisor,
    p: &Params,
    session: &mut Session<RecControl>,
) -> Result<(PathBuf, u64)> {
    let (room, events) = room::join(&supervisor.config.livekit_url, &p.token).await?;
    if session.stop.is_cancelled() {
        let _ = room.close().await;
        anyhow::bail!("stopped before the recorder joined the call");
    }
    let path = supervisor
        .config
        .data_dir
        .join("recordings")
        .join(format!("{}.wav", file_name(&p.ringid)));
    let mut wav = open_recording(&path)?;
    let mut recorder = Recorder {
        supervisor,
        ringid: p.ringid.clone(),
        roster: Roster::new(p.roster.clone()),
        transcribers: HashMap::new(),
        retired: Vec::new(),
    };
    let outcome = recorder.run(events, session, &mut wav).await;
    let _ = room.close().await;
    recorder.close_transcribers().await;
    if let Err(e) = &outcome {
        tracing::warn!(ringid = %p.ringid, "recording interrupted: {e:#}");
    }
    tracing::info!(ringid = %p.ringid, duration_ms = wav.duration_ms(), "recording finished");
    wav.finish()
}

/// Opens the call's recording file. One left behind by a worker that stopped without finishing
/// it (a crash, then ring-server reissuing the recorder) is kept, and the call continues after it.
fn open_recording(path: &Path) -> Result<WavWriter> {
    if !path.exists() {
        return WavWriter::create(path, RECORD_RATE);
    }
    let earlier = path.with_extension("earlier.wav");
    std::fs::rename(path, &earlier)?;
    let mut wav = WavWriter::create(path, RECORD_RATE)?;
    match wav.append_file(&earlier) {
        Ok(()) => {
            tracing::info!(path = %path.display(), duration_ms = wav.duration_ms(), "continuing an interrupted recording");
            let _ = std::fs::remove_file(&earlier);
        }
        Err(e) => {
            tracing::warn!(path = %earlier.display(), "could not continue an interrupted recording: {e:#}")
        }
    }
    Ok(wav)
}

struct Transcriber {
    audio: mpsc::Sender<stt::Chunk>,
    task: JoinHandle<()>,
}

struct Recorder<'a> {
    supervisor: &'a Supervisor,
    ringid: String,
    roster: Roster,
    /// Live Deepgram streams by carbon account uuid.
    transcribers: HashMap<String, Transcriber>,
    /// Streams of carbons who left, finishing their last words.
    retired: Vec<JoinHandle<()>>,
}

impl Recorder<'_> {
    async fn run(
        &mut self,
        mut events: mpsc::UnboundedReceiver<livekit::RoomEvent>,
        session: &mut Session<RecControl>,
        wav: &mut WavWriter,
    ) -> Result<()> {
        let mut feeds = Feeds::new(RECORD_RATE, Buffering::RECORDING).reporting(&self.ringid);
        let mut clock = Clock::new(RECORD_RATE);
        let mut tick = audio::tick_interval();
        loop {
            tokio::select! {
                _ = session.stop.cancelled() => break,
                _ = tick.tick() => {
                    let position_ms = clock.position_ms();
                    let (n, pulled) = feeds.pull(&mut clock, usize::MAX);
                    if n > 0 {
                        self.step(&feeds, &pulled, position_ms, n, wav)?;
                    }
                }
                Some(command) = session.control.recv() => self.command(command),
                event = events.recv() => {
                    let Some(event) = event else { break };
                    if let Change::Ended | Change::Disconnected(_) = room::apply(&mut feeds, event) {
                        tracing::info!(ringid = %self.ringid, "the room ended");
                        break;
                    }
                }
            }
        }
        // The jitter buffers still hold the last moments of the call.
        let (n, tail) = feeds.drain();
        if n > 0 {
            self.step(&feeds, &tail, clock.position_ms(), n, wav)?;
        }
        Ok(())
    }

    fn command(&mut self, command: RecControl) {
        match command {
            RecControl::Roster(roster) => {
                self.roster.replace(roster);
                self.retire_departed();
            }
            RecControl::Authority {
                account_uuid,
                identity,
            } => self.roster.set_authority(&account_uuid, &identity),
        }
    }

    fn step(
        &mut self,
        feeds: &Feeds,
        pulled: &HashMap<String, Vec<i32>>,
        position_ms: i64,
        n: usize,
        wav: &mut WavWriter,
    ) -> Result<()> {
        let mut mix = vec![0i32; n];
        for (identity, samples) in pulled {
            if self.roster.audible(identity) {
                audio::mix_into(&mut mix, samples);
            }
        }
        wav.write(&audio::clip(&mix))?;
        self.transcribe(feeds, pulled, position_ms, n);
        Ok(())
    }

    /// Feeds each carbon's authoritative audio to its own Deepgram stream. A stream starts once
    /// the carbon is heard and then follows the account across device handoffs.
    fn transcribe(
        &mut self,
        feeds: &Feeds,
        pulled: &HashMap<String, Vec<i32>>,
        position_ms: i64,
        n: usize,
    ) {
        let carbons: Vec<(String, Option<String>)> = self
            .roster
            .carbons()
            .map(|m| {
                (
                    m.uuid.clone(),
                    self.roster.resolve(&m.uuid, feeds.identities()),
                )
            })
            .collect();
        for (uuid, identity) in carbons {
            let samples = identity.as_ref().and_then(|id| pulled.get(id));
            if !self.transcribers.contains_key(&uuid) {
                if samples.is_none() {
                    continue;
                }
                let Some(transcriber) = self.start_transcriber(&uuid) else {
                    continue;
                };
                self.transcribers.insert(uuid.clone(), transcriber);
            }
            let samples = samples
                .map(|s| audio::clip(s))
                .unwrap_or_else(|| vec![0; n]);
            if let Some(t) = self.transcribers.get(&uuid)
                && t.audio
                    .try_send(stt::Chunk {
                        position_ms,
                        samples,
                    })
                    .is_err()
            {
                tracing::debug!(ringid = %self.ringid, "speech-to-text is behind; dropped 20 ms of audio");
            }
        }
    }

    fn start_transcriber(&self, account_uuid: &str) -> Option<Transcriber> {
        let key = match self.supervisor.config.deepgram_key() {
            Ok(key) => key.to_owned(),
            Err(e) => {
                tracing::warn!(ringid = %self.ringid, "no live transcription: {e}");
                return None;
            }
        };
        let (audio, rx) = mpsc::channel(STT_QUEUE);
        let task = tokio::spawn(stt::run_live(
            key,
            self.ringid.clone(),
            account_uuid.to_owned(),
            rx,
            self.supervisor.reporter.clone(),
        ));
        Some(Transcriber { audio, task })
    }

    fn retire_departed(&mut self) {
        let carbons: Vec<String> = self.roster.carbons().map(|m| m.uuid.clone()).collect();
        let departed: Vec<String> = self
            .transcribers
            .keys()
            .filter(|u| !carbons.contains(u))
            .cloned()
            .collect();
        for uuid in departed {
            if let Some(t) = self.transcribers.remove(&uuid) {
                self.retired.push(t.task);
            }
        }
    }

    /// Closes every Deepgram stream, letting each deliver its final words.
    async fn close_transcribers(&mut self) {
        let tasks = self
            .transcribers
            .drain()
            .map(|(_, t)| t.task)
            .chain(self.retired.drain(..));
        let all = futures_util::future::join_all(tasks);
        if tokio::time::timeout(Duration::from_secs(8), all)
            .await
            .is_err()
        {
            tracing::warn!(ringid = %self.ringid, "speech-to-text did not finish in time");
        }
    }
}
