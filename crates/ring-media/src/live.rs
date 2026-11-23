//! OpenAI GPT-Live over its WebSocket API, its delegation (to Ring itself, or to an OpenAI model
//! through Responses), and Natural Voice synthesis by Live readback. The model is `gpt-live-1.1`
//! unless `RING_GPT_LIVE_MODEL` names another (`gpt-live-1` speaks the same protocol).

use std::{
    sync::LazyLock,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};
use base64::{Engine, engine::general_purpose::STANDARD};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async,
    tungstenite::{Message, client::IntoClientRequest},
};

use crate::audio::{self, LIVE_RATE};

const LIVE_URL: &str = "wss://api.openai.com/v1/live/sessions";
const DEFAULT_MODEL: &str = "gpt-live-1.1";
static MODEL: LazyLock<String> = LazyLock::new(|| {
    std::env::var("RING_GPT_LIVE_MODEL")
        .ok()
        .map(|m| m.trim().to_owned())
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| DEFAULT_MODEL.to_owned())
});
const START_TIMEOUT: Duration = Duration::from_secs(20);
/// A send that cannot complete this quickly means the connection is stalled.
const SEND_TIMEOUT: Duration = Duration::from_secs(5);
/// GPT-Live streams audio continuously, silence included; this long without any event means
/// the session is gone even if the socket has not noticed yet.
pub const SILENCE_LIMIT: Duration = Duration::from_secs(20);
/// Upper bound for one `session.input_audio.append`.
const MAX_APPEND_SAMPLES: usize = LIVE_RATE as usize;
/// The Responses backend's reasoning effort.
const DELEGATION_EFFORT: &str = "medium";

/// The GPT-Live model every session uses: `RING_GPT_LIVE_MODEL`, read once, or the default.
pub fn model() -> &'static str {
    &MODEL
}

pub fn is_natural_voice(voice: &str) -> bool {
    ring_proto::natural_voices().iter().any(|v| v.id == voice)
}

pub enum Event {
    OutputAudio(Vec<i16>),
    OutputTranscript {
        delta: String,
        start_ms: i64,
        end_ms: i64,
    },
    InputTranscript {
        delta: String,
        start_ms: i64,
        end_ms: i64,
    },
    Delegation {
        id: String,
        offset_ms: Option<i64>,
    },
    /// The Responses backend called one of the client's own function tools; it waits for the
    /// result (`function_output`, then `run_backend`).
    FunctionCall {
        call_id: String,
        name: String,
        arguments: String,
    },
    /// A Responses backend response failed.
    BackendFailed {
        message: String,
    },
    /// Acknowledgement of a `session.{kind}.append` we sent with `event_id`.
    Appended {
        client_event_id: Option<String>,
    },
    Error {
        code: String,
        message: String,
        client_event_id: Option<String>,
    },
    Closed {
        reason: String,
    },
    Other,
}

pub struct Live {
    socket: WebSocketStream<MaybeTlsStream<TcpStream>>,
}

/// Delegation to Ring itself: each one is a `session.delegation.created` for the client.
pub fn client_delegation() -> Value {
    json!({"type": "client"})
}

/// Delegation to `model` through Responses, which OpenAI runs with its hosted tools (web search
/// and code interpreter) besides the client's own function `tools`.
pub fn responses_delegation(model: &str, instructions: &str, tools: Vec<Value>) -> Value {
    let mut all = vec![
        json!({"type": "web_search"}),
        // Responses requires the container even though Live's reference shows none.
        json!({"type": "code_interpreter", "container": {"type": "auto"}}),
    ];
    all.extend(tools);
    json!({
        "type": "responses",
        "responses": {
            "model": model,
            "instructions": instructions,
            "reasoning": {"effort": DELEGATION_EFFORT},
            "tools": all,
        },
    })
}

impl Live {
    /// Opens a session and returns once `session.started` arrives. `delegation` is
    /// `client_delegation()` or `responses_delegation(…)`. `opening` asks GPT-Live to speak first;
    /// it is given at startup as a developer message, which is what GPT-Live follows most reliably
    /// and soonest (a greeting appended after the start can take many seconds, or never come).
    pub async fn connect(
        api_key: &str,
        voice: &str,
        instructions: &str,
        opening: Option<&str>,
        delegation: &Value,
    ) -> Result<Self> {
        if !is_natural_voice(voice) {
            bail!("{voice:?} is not a Natural Voice");
        }
        let mut request = LIVE_URL.into_client_request()?;
        request.headers_mut().insert(
            "Authorization",
            format!("Bearer {api_key}")
                .parse()
                .context("OPENAI_API_KEY is not a valid header value")?,
        );
        let (mut socket, _) = tokio::time::timeout(START_TIMEOUT, connect_async(request))
            .await
            .context("timed out connecting to GPT-Live")?
            .context("could not connect to GPT-Live")?;
        let mut start = json!({
            "type": "session.start",
            "session": {
                "model": model(),
                "instructions": instructions,
                "audio": {"format": {"type": "audio/pcm", "rate": LIVE_RATE}, "output": {"voice": voice}},
                "delegation": delegation,
            }
        });
        if let Some(opening) = opening {
            start["session"]["input"] = json!([{
                "type": "message",
                "role": "developer",
                "content": [{"type": "input_text", "text": opening}],
            }]);
        }
        socket
            .send(Message::Text(start.to_string().into()))
            .await
            .context("GPT-Live connection lost")?;
        let started = tokio::time::timeout(START_TIMEOUT, async {
            loop {
                let event = read_json(&mut socket).await?;
                match event["type"].as_str() {
                    Some("session.started") => return Ok(()),
                    Some("error") => {
                        return Err(anyhow!(
                            "GPT-Live refused the session: {}",
                            event["error"]["code"].as_str().unwrap_or("unknown error")
                        ));
                    }
                    _ => {}
                }
            }
        })
        .await
        .context("timed out waiting for GPT-Live to start")?;
        started?;
        Ok(Self { socket })
    }

    async fn send(&mut self, event: Value) -> Result<()> {
        let message = Message::Text(event.to_string().into());
        tokio::time::timeout(SEND_TIMEOUT, self.socket.send(message))
            .await
            .context("GPT-Live connection stalled")?
            .context("GPT-Live connection lost")
    }

    pub async fn append_audio(&mut self, samples: &[i16]) -> Result<()> {
        for chunk in samples.chunks(MAX_APPEND_SAMPLES) {
            let audio = STANDARD.encode(audio::pcm_bytes(chunk));
            self.send(json!({"type": "session.input_audio.append", "audio": audio}))
                .await?;
        }
        Ok(())
    }

    /// Gives the Responses backend the result of its function call `call_id`. It continues only
    /// once every call it made has a result and `run_backend` is sent.
    pub async fn function_output(
        &mut self,
        call_id: &str,
        output: &str,
        event_id: Option<&str>,
    ) -> Result<()> {
        let item = json!({"type": "function_call_output", "call_id": call_id, "output": output});
        self.create_item(item, event_id).await
    }

    /// Adds a message to the Responses backend's conversation, for what it does next. It does
    /// not stop work under way.
    pub async fn backend_message(&mut self, text: &str, event_id: Option<&str>) -> Result<()> {
        let item = json!({
            "type": "message",
            "role": "user",
            "content": [{"type": "input_text", "text": text}],
        });
        self.create_item(item, event_id).await
    }

    async fn create_item(&mut self, item: Value, event_id: Option<&str>) -> Result<()> {
        let mut event = json!({"type": "response.item.create", "item": item});
        if let Some(id) = event_id {
            event["event_id"] = json!(id);
        }
        self.send(event).await
    }

    /// Lets the Responses backend continue with the function results it was given.
    pub async fn run_backend(&mut self) -> Result<()> {
        self.send(json!({"type": "response.create"})).await
    }

    /// `kind` is `instructions`, `thinking` or `commentary`.
    pub async fn append(
        &mut self,
        kind: &str,
        content: &str,
        delegation_id: Option<&str>,
        event_id: Option<&str>,
    ) -> Result<()> {
        let mut event = json!({
            "type": format!("session.{kind}.append"),
            "delegation_id": delegation_id,
            "content": content,
        });
        if let Some(id) = event_id {
            event["event_id"] = json!(id);
        }
        self.send(event).await
    }

    /// Next event from the session. Cancel-safe: dropping the future loses nothing.
    pub async fn next_event(&mut self) -> Result<Event> {
        let event = read_json(&mut self.socket).await?;
        let text = |key: &str| event[key].as_str().unwrap_or_default().to_owned();
        let ms = |key: &str| event[key].as_i64().unwrap_or_default();
        Ok(match event["type"].as_str().unwrap_or_default() {
            "session.output_audio.delta" => {
                let bytes = STANDARD.decode(event["delta"].as_str().unwrap_or_default())?;
                Event::OutputAudio(audio::pcm_samples(&bytes))
            }
            "session.output_transcript.delta" => Event::OutputTranscript {
                delta: text("delta"),
                start_ms: ms("start_ms"),
                end_ms: ms("end_ms"),
            },
            "session.input_transcript.delta" => Event::InputTranscript {
                delta: text("delta"),
                start_ms: ms("start_ms"),
                end_ms: ms("end_ms"),
            },
            "session.delegation.created" => match event["delegation"]["id"].as_str() {
                Some(id)
                    if event["delegation"]["target"]
                        .as_str()
                        .is_none_or(|t| t == "client") =>
                {
                    Event::Delegation {
                        id: id.to_owned(),
                        offset_ms: event["offset_ms"].as_i64(),
                    }
                }
                _ => Event::Other,
            },
            // The Responses backend's own streaming events: only its calls to the client's
            // functions, and its failures, concern the client.
            "response.event" => {
                let inner = &event["event"];
                let item = &inner["item"];
                match inner["type"].as_str().unwrap_or_default() {
                    "response.output_item.done" if item["type"] == "function_call" => {
                        Event::FunctionCall {
                            call_id: item["call_id"].as_str().unwrap_or_default().to_owned(),
                            name: item["name"].as_str().unwrap_or_default().to_owned(),
                            arguments: item["arguments"].as_str().unwrap_or_default().to_owned(),
                        }
                    }
                    "response.failed" => Event::BackendFailed {
                        message: inner["response"]["error"]["message"]
                            .as_str()
                            .unwrap_or("unknown error")
                            .to_owned(),
                    },
                    "error" => Event::BackendFailed {
                        message: inner["message"]
                            .as_str()
                            .or(inner["error"]["message"].as_str())
                            .unwrap_or("unknown error")
                            .to_owned(),
                    },
                    _ => Event::Other,
                }
            }
            "session.thinking.appended"
            | "session.commentary.appended"
            | "session.instructions.appended" => Event::Appended {
                client_event_id: event["client_event_id"].as_str().map(str::to_owned),
            },
            "error" => Event::Error {
                code: event["error"]["code"]
                    .as_str()
                    .or(event["error"]["type"].as_str())
                    .unwrap_or("unknown")
                    .to_owned(),
                message: event["error"]["message"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
                client_event_id: event["error"]["client_event_id"]
                    .as_str()
                    .map(str::to_owned),
            },
            "session.closed" => Event::Closed {
                reason: event["reason"].as_str().unwrap_or("unknown").to_owned(),
            },
            _ => Event::Other,
        })
    }

    /// Ends the session gracefully, waiting briefly for `session.closed`.
    pub async fn close(mut self) {
        if self.send(json!({"type": "session.close"})).await.is_ok() {
            let _ = tokio::time::timeout(Duration::from_secs(8), async {
                while let Ok(event) = read_json(&mut self.socket).await {
                    if event["type"] == "session.closed" {
                        break;
                    }
                }
            })
            .await;
        }
        let _ = tokio::time::timeout(SEND_TIMEOUT, self.socket.close(None)).await;
    }
}

async fn read_json(socket: &mut WebSocketStream<MaybeTlsStream<TcpStream>>) -> Result<Value> {
    loop {
        match socket.next().await {
            Some(Ok(Message::Text(text))) => {
                return serde_json::from_str(&text).context("malformed GPT-Live event");
            }
            Some(Ok(Message::Close(frame))) => {
                bail!(
                    "GPT-Live closed the connection{}",
                    frame.map(|f| format!(" ({})", f.code)).unwrap_or_default()
                )
            }
            Some(Ok(_)) => {}
            Some(Err(e)) => return Err(anyhow!(e).context("GPT-Live connection lost")),
            None => bail!("GPT-Live connection lost"),
        }
    }
}

/// Live readback emits up to this much text per session; longer text is split at whitespace.
const READBACK_CHARS: usize = 400;
/// GPT-Live has no end-of-speech event; this much audible-free output ends an utterance.
const READBACK_QUIET: Duration = Duration::from_secs(2);
const READBACK_TIMEOUT: Duration = Duration::from_secs(60);
const READBACK_ATTEMPTS: usize = 3;
pub const AUDIBLE: i16 = 250;

/// Speaks `text` in `voice` and returns 24 kHz PCM. The words GPT-Live reports saying must
/// match the requested text, otherwise the audio is rejected rather than risk a voicemail or
/// greeting that says something else.
pub async fn synthesize(api_key: &str, voice: &str, text: &str) -> Result<Vec<i16>> {
    if text.trim().is_empty() {
        bail!("nothing to synthesize");
    }
    if !is_natural_voice(voice) {
        bail!("{voice:?} is not a Natural Voice");
    }
    let mut out = Vec::new();
    for chunk in split_text(text, READBACK_CHARS) {
        let mut last_error = None;
        let mut spoken = None;
        for attempt in 1..=READBACK_ATTEMPTS {
            match readback(api_key, voice, &chunk).await {
                Ok(samples) => {
                    spoken = Some(samples);
                    break;
                }
                Err(e) => {
                    tracing::warn!(attempt, "Natural Voice readback failed: {e:#}");
                    last_error = Some(e);
                }
            }
        }
        let samples =
            spoken.ok_or_else(|| last_error.unwrap_or_else(|| anyhow!("readback failed")))?;
        if !out.is_empty() {
            out.extend(audio::silence(LIVE_RATE, Duration::from_millis(250)));
        }
        out.extend(samples);
    }
    Ok(out)
}

async fn readback(api_key: &str, voice: &str, text: &str) -> Result<Vec<i16>> {
    let mut live = Live::connect(
        api_key,
        voice,
        "Read supplied messages verbatim. Do not add any words or commentary.",
        Some(&format!(
            "Speak first. Read exactly this message now, then stay silent: {text}"
        )),
        // A readback never delegates.
        &client_delegation(),
    )
    .await?;
    let mut samples: Vec<i16> = Vec::new();
    let mut transcript = String::new();
    let mut last_audible: Option<Instant> = None;
    let mut tick = audio::tick_interval();
    let silence = vec![0i16; (LIVE_RATE / 50) as usize];
    let result = tokio::time::timeout(READBACK_TIMEOUT, async {
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    if last_audible.is_some_and(|t| t.elapsed() > READBACK_QUIET) {
                        return Ok(());
                    }
                    live.append_audio(&silence).await?;
                }
                event = live.next_event() => match event? {
                    Event::OutputAudio(chunk) => {
                        if samples.len() + chunk.len() > LIVE_RATE as usize * 60 {
                            bail!("readback ran too long");
                        }
                        if chunk.iter().any(|s| s.unsigned_abs() > AUDIBLE as u16) {
                            last_audible = Some(Instant::now());
                        }
                        samples.extend(chunk);
                    }
                    Event::OutputTranscript { delta, .. } => transcript.push_str(&delta),
                    Event::Error { code, .. } => bail!("GPT-Live readback error: {code}"),
                    Event::Closed { reason } => bail!("GPT-Live ended the readback: {reason}"),
                    _ => {}
                }
            }
        }
    })
    .await
    .unwrap_or_else(|_| {
        Err(anyhow!(
            "readback did not finish in {} s",
            READBACK_TIMEOUT.as_secs()
        ))
    });
    // The audio is complete; the session winds down in the background.
    tokio::spawn(live.close());
    result?;
    if normalize(&transcript) != normalize(text) {
        bail!("the spoken words did not match the requested text");
    }
    let trimmed = audio::trim_silence(
        &samples,
        LIVE_RATE,
        AUDIBLE,
        Duration::from_millis(120),
        Duration::from_millis(350),
    );
    if trimmed.is_empty() {
        bail!("readback produced no audible speech");
    }
    Ok(trimmed)
}

/// The words in a GPT-Live transcript. GPT-Live marks non-speech sounds in brackets, such as
/// "[hum]"; nobody said those, so they are dropped. Whitespace is collapsed.
pub fn spoken(transcript: &str) -> String {
    let mut words = String::with_capacity(transcript.len());
    let mut depth = 0u32;
    for c in transcript.chars() {
        match c {
            '[' => depth += 1,
            ']' if depth > 0 => depth -= 1,
            c if depth == 0 => words.push(c),
            _ => {}
        }
    }
    words.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn normalize(text: &str) -> String {
    spoken(text)
        .split_whitespace()
        .map(|word| {
            word.chars()
                .filter(|c| c.is_alphanumeric())
                .flat_map(char::to_lowercase)
                .collect::<String>()
        })
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn split_text(text: &str, limit: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut rest = text.trim();
    while !rest.is_empty() {
        let boundary = rest
            .char_indices()
            .nth(limit)
            .map_or(rest.len(), |(i, _)| i);
        let end = if boundary < rest.len() {
            rest[..boundary]
                .rfind(char::is_whitespace)
                .filter(|&i| i > 0)
                .unwrap_or(boundary)
        } else {
            boundary
        };
        let (chunk, next) = rest.split_at(end);
        if !chunk.trim().is_empty() {
            chunks.push(chunk.trim().to_owned());
        }
        rest = next.trim_start();
    }
    chunks
}
