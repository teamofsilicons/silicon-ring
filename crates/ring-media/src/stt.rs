//! Deepgram speech-to-text: one live socket per carbon on a call, and prerecorded voicemail.

use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};
use futures_util::{SinkExt, StreamExt};
use ring_proto::internal::FromMedia;
use serde_json::Value;
use tokio::{net::TcpStream, sync::mpsc};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async,
    tungstenite::{Message, client::IntoClientRequest},
};

use crate::{audio, link::Reporter};

const LIVE_URL: &str = "wss://api.deepgram.com/v1/listen?model=nova-3&language=multi&encoding=linear16&sample_rate=16000&channels=1&interim_results=true&smart_format=true&punctuate=true";
const PRERECORDED_URL: &str = "https://api.deepgram.com/v1/listen";
/// Audio kept while reconnecting, so a short outage loses no words.
const MAX_BACKLOG: Duration = Duration::from_secs(20);
/// Finalize an utterance when Deepgram has said nothing more for this long.
const IDLE_FINAL: Duration = Duration::from_millis(2500);
/// Deepgram ends speech at every short pause; sentences closer together than this stay one entry.
const MERGE_WINDOW: Duration = Duration::from_millis(900);
const PARTIAL_INTERVAL: Duration = Duration::from_millis(250);

/// Audio for one account, positioned on the recorder's timeline.
pub struct Chunk {
    pub position_ms: i64,
    pub samples: Vec<i16>,
}

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Streams one carbon's audio to Deepgram until `audio` closes, reconnecting on errors, and
/// reports the transcript as that carbon's speech.
pub async fn run_live(
    api_key: String,
    ringid: String,
    account_uuid: String,
    mut audio: mpsc::Receiver<Chunk>,
    reporter: Reporter,
) {
    let mut speech = Utterances::new(ringid, account_uuid.clone(), reporter);
    let mut backlog: VecDeque<Chunk> = VecDeque::new();
    let mut failures = 0u32;
    loop {
        let socket = {
            let connecting = connect(&api_key);
            tokio::pin!(connecting);
            loop {
                tokio::select! {
                    socket = &mut connecting => break Some(socket),
                    closed = buffer_while(&mut audio, &mut backlog) => if closed { break None; },
                }
            }
        };
        let Some(socket) = socket else { break };
        let mut socket = match socket {
            Ok(socket) => socket,
            Err(e) => {
                failures += 1;
                tracing::warn!(account = %account_uuid, "Deepgram connect failed: {e:#}");
                let delay = backoff(failures);
                if wait_buffering(&mut audio, &mut backlog, delay).await {
                    break;
                }
                continue;
            }
        };
        let mut base_ms: Option<i64> = None;
        let opened = Instant::now();
        match stream(
            &mut socket,
            &mut audio,
            &mut backlog,
            &mut base_ms,
            &mut speech,
        )
        .await
        {
            Ok(()) => {
                speech.finish();
                return;
            }
            Err(e) => {
                if opened.elapsed() > Duration::from_secs(30) {
                    failures = 0;
                }
                failures += 1;
                tracing::warn!(account = %account_uuid, "Deepgram stream interrupted: {e:#}");
                speech.finish();
                let _ = tokio::time::timeout(Duration::from_secs(3), socket.close(None)).await;
                if wait_buffering(&mut audio, &mut backlog, backoff(failures)).await {
                    break;
                }
            }
        }
    }
    speech.finish();
}

fn backoff(failures: u32) -> Duration {
    Duration::from_millis((250u64 << failures.min(5)).min(8000))
}

async fn connect(api_key: &str) -> Result<Socket> {
    let mut request = LIVE_URL.into_client_request()?;
    request.headers_mut().insert(
        "Authorization",
        format!("Token {api_key}")
            .parse()
            .context("DEEPGRAM_API_KEY is not a valid header value")?,
    );
    let (socket, _) = tokio::time::timeout(Duration::from_secs(15), connect_async(request))
        .await
        .context("timed out connecting to Deepgram")?
        .context("could not connect to Deepgram")?;
    Ok(socket)
}

fn keep(backlog: &mut VecDeque<Chunk>, chunk: Chunk) {
    backlog.push_back(chunk);
    while backlog
        .front()
        .zip(backlog.back())
        .is_some_and(|(first, last)| {
            Duration::from_millis((last.position_ms - first.position_ms).max(0) as u64)
                > MAX_BACKLOG
        })
    {
        backlog.pop_front();
    }
}

/// Buffers audio until the channel yields nothing new. Returns true once the channel closed.
async fn buffer_while(audio: &mut mpsc::Receiver<Chunk>, backlog: &mut VecDeque<Chunk>) -> bool {
    match audio.recv().await {
        Some(chunk) => {
            keep(backlog, chunk);
            false
        }
        None => true,
    }
}

async fn wait_buffering(
    audio: &mut mpsc::Receiver<Chunk>,
    backlog: &mut VecDeque<Chunk>,
    delay: Duration,
) -> bool {
    let deadline = tokio::time::Instant::now() + delay;
    loop {
        tokio::select! {
            _ = tokio::time::sleep_until(deadline) => return false,
            closed = buffer_while(audio, backlog) => if closed { return true; },
        }
    }
}

/// Runs one socket. Returns Ok once the audio channel closed and Deepgram flushed its results.
async fn stream(
    socket: &mut Socket,
    audio: &mut mpsc::Receiver<Chunk>,
    backlog: &mut VecDeque<Chunk>,
    base_ms: &mut Option<i64>,
    speech: &mut Utterances,
) -> Result<()> {
    while let Some(chunk) = backlog.pop_front() {
        send_audio(socket, &chunk, base_ms).await?;
    }
    let mut tick = tokio::time::interval(Duration::from_millis(250));
    let mut closing: Option<tokio::time::Instant> = None;
    loop {
        tokio::select! {
            chunk = audio.recv(), if closing.is_none() => match chunk {
                Some(chunk) => send_audio(socket, &chunk, base_ms).await?,
                None => {
                    // Ask Deepgram for the final results of what it already has.
                    socket.send(Message::Text(r#"{"type":"CloseStream"}"#.into())).await?;
                    closing = Some(tokio::time::Instant::now() + Duration::from_secs(5));
                }
            },
            message = socket.next() => {
                let base_ms = base_ms.unwrap_or_default();
                if on_message(message, closing.is_some(), speech, base_ms)? {
                    return Ok(());
                }
            }
            _ = tick.tick() => {
                if closing.is_some_and(|deadline| tokio::time::Instant::now() >= deadline) {
                    return Ok(());
                }
                speech.idle();
            }
        }
    }
}

/// Handles one message from Deepgram. Returns true when the stream is finished.
fn on_message(
    message: Option<Result<Message, tokio_tungstenite::tungstenite::Error>>,
    closing: bool,
    speech: &mut Utterances,
    base_ms: i64,
) -> Result<bool> {
    match message {
        Some(Ok(Message::Text(text))) => {
            let event: Value = serde_json::from_str(&text).context("malformed Deepgram message")?;
            match event["type"].as_str() {
                Some("Results") => speech.result(&event, base_ms),
                // Deepgram's last message after CloseStream.
                Some("Metadata") if closing => return Ok(true),
                Some("Error") => bail!(
                    "Deepgram error: {}",
                    event["description"].as_str().unwrap_or("unknown")
                ),
                _ => {}
            }
            Ok(false)
        }
        Some(Ok(Message::Close(_))) | None if closing => Ok(true),
        Some(Ok(Message::Close(_))) | None => bail!("Deepgram closed the stream"),
        Some(Ok(_)) => Ok(false),
        Some(Err(e)) => Err(anyhow!(e)),
    }
}

async fn send_audio(socket: &mut Socket, chunk: &Chunk, base_ms: &mut Option<i64>) -> Result<()> {
    base_ms.get_or_insert(chunk.position_ms);
    let message = Message::Binary(audio::pcm_bytes(&chunk.samples).into());
    tokio::time::timeout(Duration::from_secs(5), socket.send(message))
        .await
        .context("Deepgram connection stalled")?
        .context("Deepgram connection lost")
}

/// Turns Deepgram's interim and final results into Ring speech segments: one segment per
/// utterance, with interim text as partials.
struct Utterances {
    ringid: String,
    account_uuid: String,
    reporter: Reporter,
    segment_id: String,
    finals: Vec<String>,
    start_ms: Option<i64>,
    last_result: Instant,
    /// When Deepgram last marked the end of speech, if nothing was said since.
    ended: Option<Instant>,
    last_partial: Option<Instant>,
}

impl Utterances {
    fn new(ringid: String, account_uuid: String, reporter: Reporter) -> Self {
        Self {
            ringid,
            account_uuid,
            reporter,
            segment_id: new_segment_id(),
            finals: Vec::new(),
            start_ms: None,
            last_result: Instant::now(),
            ended: None,
            last_partial: None,
        }
    }

    fn result(&mut self, event: &Value, base_ms: i64) {
        let alternative = &event["channel"]["alternatives"][0];
        let text = alternative["transcript"]
            .as_str()
            .unwrap_or_default()
            .trim();
        let is_final = event["is_final"].as_bool().unwrap_or(false);
        let speech_final = event["speech_final"].as_bool().unwrap_or(false);
        if !text.is_empty() {
            self.last_result = Instant::now();
            self.ended = None;
            // A result's own start is the start of the audio window it covers, which often
            // begins with the silence before the first word.
            let start_s = alternative["words"][0]["start"]
                .as_f64()
                .or(event["start"].as_f64())
                .unwrap_or_default();
            self.start_ms
                .get_or_insert(base_ms + (start_s * 1000.0) as i64);
            if is_final {
                self.finals.push(text.to_owned());
                self.partial(None, true);
            } else {
                self.partial(Some(text), false);
            }
        }
        if is_final && speech_final {
            self.ended = Some(Instant::now());
        }
    }

    fn partial(&mut self, interim: Option<&str>, force: bool) {
        if !force
            && self
                .last_partial
                .is_some_and(|t| t.elapsed() < PARTIAL_INTERVAL)
        {
            return;
        }
        let mut text = self.finals.join(" ");
        if let Some(interim) = interim {
            if !text.is_empty() {
                text.push(' ');
            }
            text.push_str(interim);
        }
        if text.is_empty() {
            return;
        }
        self.last_partial = Some(Instant::now());
        self.reporter.send(FromMedia::SpeechPartial {
            ringid: self.ringid.clone(),
            account_uuid: self.account_uuid.clone(),
            segment_id: self.segment_id.clone(),
            text,
        });
    }

    fn idle(&mut self) {
        let Some(last) = self.finals.last() else {
            return;
        };
        // Deepgram marks the end of speech at the shortest pause, and the words after it can
        // take a second to arrive; mid-sentence ("Also, …") wait as long as for silence.
        let window = if last.ends_with(['.', '?', '!', '…', '。', '？', '！']) {
            MERGE_WINDOW
        } else {
            IDLE_FINAL
        };
        let ended = self.ended.is_some_and(|t| t.elapsed() >= window);
        if ended || self.last_result.elapsed() >= IDLE_FINAL {
            self.finish();
        }
    }

    /// Emits the utterance so far as final and starts a new one.
    fn finish(&mut self) {
        if !self.finals.is_empty() {
            self.reporter.send(FromMedia::SpeechFinal {
                ringid: self.ringid.clone(),
                account_uuid: self.account_uuid.clone(),
                segment_id: self.segment_id.clone(),
                text: self.finals.join(" "),
                offset_ms: self.start_ms.map(|ms| ms.max(0)),
            });
        }
        self.segment_id = new_segment_id();
        self.finals.clear();
        self.start_ms = None;
        self.last_partial = None;
        self.ended = None;
    }
}

pub fn new_segment_id() -> String {
    format!("seg_{}", uuid::Uuid::new_v4().simple())
}

/// Transcribes a recorded WAV with Deepgram's prerecorded API.
pub async fn transcribe(http: &reqwest::Client, api_key: &str, wav: Vec<u8>) -> Result<String> {
    let response = http
        .post(PRERECORDED_URL)
        .query(&[
            ("model", "nova-3"),
            ("language", "multi"),
            ("smart_format", "true"),
            ("punctuate", "true"),
        ])
        .header("Authorization", format!("Token {api_key}"))
        .header("Content-Type", "audio/wav")
        .body(wav)
        .timeout(Duration::from_secs(120))
        .send()
        .await
        .context("could not reach Deepgram")?;
    let status = response.status();
    if !status.is_success() {
        bail!("Deepgram returned {status}");
    }
    let body: Value = response
        .json()
        .await
        .context("malformed Deepgram response")?;
    body["results"]["channels"][0]["alternatives"][0]["transcript"]
        .as_str()
        .map(|t| t.trim().to_owned())
        .context("Deepgram response has no transcript")
}
