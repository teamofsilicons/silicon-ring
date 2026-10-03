use crate::{checked, http, required_env, Error, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::time::Duration;
use tokio::net::TcpStream;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{client::IntoClientRequest, Message},
    MaybeTlsStream, WebSocketStream,
};

pub const NATURAL_VOICES: &[&str] = &[
    "ripple", "vesper", "willow", "stone", "gleam", "meridian", "bossa", "tempo",
];
pub const DEFAULT_VOICE: &str = "gleam";
pub const TTS_VOICES: &[&str] = &[
    "alloy", "echo", "fable", "onyx", "nova", "shimmer", "coral", "verse", "ballad", "ash", "sage",
    "marin", "cedar",
];
pub type LiveSocket = WebSocketStream<MaybeTlsStream<TcpStream>>;

#[derive(Clone)]
pub struct OpenAi {
    http: reqwest::Client,
    key: String,
}
impl OpenAi {
    pub fn new(key: String) -> Result<Self> {
        Ok(Self { http: http()?, key })
    }
    pub fn from_env() -> Result<Self> {
        Self::new(required_env("OPENAI_API_KEY")?)
    }
    /// Starts the actual GPT-Live model, with the Silicon as client-delegated backend.
    /// Returns only after session.started. The caller must continuously receive events.
    pub async fn connect_live(
        &self,
        voice: &str,
        instructions: &str,
        start: Option<&str>,
    ) -> Result<LiveSocket> {
        let event = live_start_event(voice, instructions)?;
        if start.is_some_and(|text| text.chars().count() > 100) {
            return Err(Error::new(
                "INVALID_START",
                "Start must be at most 100 characters.",
                false,
            ));
        }
        let mut request = "wss://api.openai.com/v1/live/sessions"
            .into_client_request()
            .map_err(|_| Error::network("OpenAI"))?;
        request.headers_mut().insert(
            "Authorization",
            format!("Bearer {}", self.key).parse().map_err(|_| {
                Error::new("PROVIDER_AUTH_FAILED", "OpenAI key is malformed.", false)
            })?,
        );
        let (mut socket, _) = tokio::time::timeout(Duration::from_secs(20), connect_async(request))
            .await
            .map_err(|_| Error::network("OpenAI Live"))?
            .map_err(|_| Error::network("OpenAI Live"))?;
        send_event(&mut socket, event).await?;
        tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                let event = receive_event(&mut socket).await?;
                match event["type"].as_str() {
                    Some("session.started") => return Ok(()),
                    Some("error") => {
                        return Err(Error::new(
                            "LIVE_START_FAILED",
                            "OpenAI rejected GPT-Live startup; check model access and voice.",
                            false,
                        ))
                    }
                    _ => {}
                }
            }
        })
        .await
        .map_err(|_| Error::network("OpenAI Live startup"))??;
        if let Some(start) = start.filter(|s| !s.is_empty()) {
            if start.chars().count() > 100 {
                return Err(Error::new(
                    "INVALID_START",
                    "Start must be at most 100 characters.",
                    false,
                ));
            }
            send_event(
                &mut socket,
                update_event(
                    "instructions",
                    &format!("Speak first. Begin by saying: {start}"),
                    None,
                )?,
            )
            .await?;
        }
        Ok(socket)
    }
    /// A bounded, same-voice Live readback. The transcript must match the requested words before audio is accepted.
    /// GPT-Live has no end-of-speech event: two seconds of audio inactivity closes the utterance.
    /// This is deliberately reported as Live synthesis, not the separate TTS endpoint.
    pub async fn synthesize_natural_speech(&self, text: &str, voice: &str) -> Result<Vec<u8>> {
        if text.trim().is_empty() || text.chars().count() > 400 {
            return Err(Error::new(
                "INVALID_TEXT",
                "Natural-voice message must contain 1–400 characters.",
                false,
            ));
        }
        let mut socket = self
            .connect_live(
                voice,
                "Read supplied messages verbatim. Do not add any words or commentary.",
                None,
            )
            .await?;
        send_event(
            &mut socket,
            update_event(
                "instructions",
                &format!("Speak first. Read exactly this message, then stay silent: {text}"),
                None,
            )?,
        )
        .await?;
        let mut samples = Vec::new();
        let mut transcript = String::new();
        let mut last_output = None;
        let mut tick = tokio::time::interval(Duration::from_millis(20));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let readback=tokio::time::timeout(Duration::from_secs(60),async {
            loop {
                tokio::select! {
                    _=tick.tick()=>{
                        if last_output.is_some_and(|t:tokio::time::Instant|t.elapsed()>Duration::from_secs(2)) {break;}
                        send_event(&mut socket,audio_event(&[0;960])?).await?;
                    }
                    event=receive_event(&mut socket)=>{
                        let event=event?;
                        match event["type"].as_str() {
                            Some("session.output_audio.delta")=>{
                                let delta=event["delta"].as_str().ok_or_else(||Error::network("OpenAI speech"))?;
                                let bytes=STANDARD.decode(delta).map_err(|_|Error::network("OpenAI speech"))?;
                                if bytes.len()%2!=0 || samples.len()+bytes.len()>2_880_000 {return Err(Error::network("OpenAI speech"));}
                                samples.extend_from_slice(&bytes);
                                // Live continuously emits silence too; completion follows audible samples, not WS inactivity.
                                if bytes.chunks_exact(2).any(|s|i16::from_le_bytes([s[0],s[1]]).unsigned_abs()>250){last_output=Some(tokio::time::Instant::now());}
                            }
                            Some("session.output_transcript.delta")=>transcript.push_str(event["delta"].as_str().unwrap_or("")),
                            Some("error")=>return Err(Error::new("LIVE_SYNTHESIS_FAILED","OpenAI rejected the Natural Voice readback.",false)),
                            _=>{}
                        }
                    }
                }
            }
            Ok(())
        }).await.unwrap_or_else(|_|Err(Error::new("LIVE_SYNTHESIS_TIMEOUT","Natural Voice readback did not complete in 60 seconds.",true)));
        let finalized = close_live(&mut socket).await;
        readback?;
        finalized?;
        if samples.is_empty() || normalize_speech(&transcript) != normalize_speech(text) {
            return Err(Error::new(
                "SPEECH_VERIFICATION_FAILED",
                "Generated words did not match the requested message; audio was not accepted.",
                true,
            ));
        }
        Ok(pcm_wav(&samples))
    }
    /// The speech endpoint currently does not support GPT-Live Natural Voices. Never substitute.
    pub async fn tts(&self, text: &str, voice: &str) -> Result<Vec<u8>> {
        if !TTS_VOICES.contains(&voice) {
            return Err(Error::new("VOICE_UNAVAILABLE", "OpenAI TTS does not support this GPT-Live Natural Voice. Matching-voice voicemail synthesis is unavailable; no other voice was substituted.", false));
        }
        if text.trim().is_empty() || text.chars().count() > 4096 {
            return Err(Error::new(
                "INVALID_TEXT",
                "Speech text must contain 1–4096 characters.",
                false,
            ));
        }
        let response = self.http.post("https://api.openai.com/v1/audio/speech").bearer_auth(&self.key)
            .json(&json!({"model":"gpt-4o-mini-tts","voice":voice,"input":text,"response_format":"wav"}))
            .send().await.map_err(|_| Error::network("OpenAI TTS"))?;
        checked(response, "OpenAI TTS")
            .await?
            .bytes()
            .await
            .map(|v| v.to_vec())
            .map_err(|_| Error::network("OpenAI TTS"))
    }
}
pub fn live_start_event(voice: &str, instructions: &str) -> Result<Value> {
    if !NATURAL_VOICES.contains(&voice) {
        return Err(Error::new(
            "INVALID_VOICE",
            "Select one of Ring's supported Natural Voices.",
            false,
        ));
    }
    if instructions.chars().count() > 400 {
        return Err(Error::new(
            "INVALID_CONTEXT",
            "Representative context must be at most 400 characters.",
            false,
        ));
    }
    Ok(
        json!({"type":"session.start","session":{"model":"gpt-live-1","instructions":instructions,"audio":{"format":{"type":"audio/pcm","rate":24000},"output":{"voice":voice}},"delegation":{"type":"client"}}}),
    )
}
pub fn audio_event(pcm: &[u8]) -> Result<Value> {
    if pcm.is_empty() || pcm.len() % 2 != 0 || pcm.len() > 48_000 {
        return Err(Error::new(
            "INVALID_AUDIO",
            "Send 1–1000ms of complete PCM16 mono samples at 24kHz.",
            false,
        ));
    }
    Ok(json!({"type":"session.input_audio.append","audio":STANDARD.encode(pcm)}))
}
pub fn update_event(kind: &str, content: &str, delegation_id: Option<&str>) -> Result<Value> {
    if !["thinking", "commentary", "instructions"].contains(&kind)
        || content.is_empty()
        || content.chars().count() > if kind == "instructions" { 500 } else { 160 }
    {
        return Err(Error::new(
            "INVALID_UPDATE",
            "Use thinking/commentary with 1–160 characters, or instructions with 1–500 characters.",
            false,
        ));
    }
    Ok(
        json!({"type":format!("session.{kind}.append"),"delegation_id":delegation_id,"content":content}),
    )
}
pub async fn send_event(socket: &mut LiveSocket, event: Value) -> Result<()> {
    socket
        .send(Message::Text(event.to_string().into()))
        .await
        .map_err(|_| Error::network("OpenAI Live"))
}
pub async fn receive_event(socket: &mut LiveSocket) -> Result<Value> {
    loop {
        match socket.next().await {
            Some(Ok(Message::Text(text))) => {
                return serde_json::from_str(&text).map_err(|_| {
                    Error::new(
                        "INVALID_PROVIDER_EVENT",
                        "OpenAI returned malformed event JSON.",
                        true,
                    )
                })
            }
            Some(Ok(Message::Ping(data))) => socket
                .send(Message::Pong(data))
                .await
                .map_err(|_| Error::network("OpenAI Live"))?,
            Some(Ok(Message::Close(_))) | None => return Err(Error::network("OpenAI Live")),
            Some(Err(_)) => return Err(Error::network("OpenAI Live")),
            _ => {}
        }
    }
}
/// Drain the terminal event to retain confirmed final usage; never silently call a socket close a finalized session.
pub async fn close_live(socket: &mut LiveSocket) -> Result<Value> {
    send_event(socket, json!({"type":"session.close"})).await?;
    let usage = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let event = receive_event(socket).await?;
            if event["type"] == "session.closed" {
                return Ok(event["usage"].clone());
            }
        }
    })
    .await
    .map_err(|_| {
        Error::new(
            "LIVE_FINALIZATION_UNCONFIRMED",
            "OpenAI did not confirm final usage before timeout.",
            true,
        )
    })??;
    socket
        .close(None)
        .await
        .map_err(|_| Error::network("OpenAI Live"))?;
    Ok(usage)
}

#[derive(Clone)]
pub struct Deepgram {
    http: reqwest::Client,
    key: String,
}
impl Deepgram {
    pub fn new(key: String) -> Result<Self> {
        Ok(Self { http: http()?, key })
    }
    pub fn from_env() -> Result<Self> {
        Self::new(required_env("DEEPGRAM_API_KEY")?)
    }
    /// Binary PCM16 mono 24kHz. Send KeepAlive during silence and CloseStream before shutdown.
    pub async fn connect_stream(&self) -> Result<LiveSocket> {
        let mut request="wss://api.deepgram.com/v1/listen?model=nova-3&language=multi&encoding=linear16&sample_rate=24000&channels=1&interim_results=true&punctuate=true".into_client_request().map_err(|_|Error::network("Deepgram"))?;
        request.headers_mut().insert(
            "Authorization",
            format!("Token {}", self.key).parse().map_err(|_| {
                Error::new("PROVIDER_AUTH_FAILED", "Deepgram key is malformed.", false)
            })?,
        );
        let (socket, _) = tokio::time::timeout(Duration::from_secs(15), connect_async(request))
            .await
            .map_err(|_| Error::network("Deepgram stream"))?
            .map_err(|_| Error::network("Deepgram stream"))?;
        Ok(socket)
    }
    pub async fn transcribe(&self, audio: Vec<u8>, content_type: &str) -> Result<Value> {
        if audio.is_empty() || audio.len() > 100 * 1024 * 1024 {
            return Err(Error::new(
                "INVALID_AUDIO",
                "Voicemail audio must contain 1 byte–100MiB.",
                false,
            ));
        }
        if ![
            "audio/wav",
            "audio/webm",
            "audio/ogg",
            "audio/mpeg",
            "audio/mp4",
        ]
        .contains(&content_type)
        {
            return Err(Error::new(
                "INVALID_AUDIO",
                "Unsupported voicemail audio container.",
                false,
            ));
        }
        let response = self
            .http
            .post("https://api.deepgram.com/v1/listen")
            .query(&[
                ("model", "nova-3"),
                ("language", "multi"),
                ("smart_format", "true"),
                ("punctuate", "true"),
            ])
            .header("Authorization", format!("Token {}", self.key))
            .header("Content-Type", content_type)
            .body(audio)
            .send()
            .await
            .map_err(|_| Error::network("Deepgram"))?;
        checked(response, "Deepgram")
            .await?
            .json()
            .await
            .map_err(|_| Error::network("Deepgram"))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn live_wire_contract_and_input_boundaries() {
        let event = live_start_event("gleam", "Keep replies concise.").unwrap();
        assert_eq!(event["session"]["model"], "gpt-live-1");
        assert_eq!(event["session"]["delegation"]["type"], "client");
        assert!(live_start_event("marin", "").is_err());
        assert!(audio_event(&[0]).is_err());
        assert_eq!(audio_event(&[0, 0]).unwrap()["audio"], "AAA=");
        assert!(update_event("commentary", &"🙂".repeat(161), None).is_err());
        assert_eq!(
            update_event("thinking", "checking", None).unwrap()["delegation_id"],
            Value::Null
        );
        assert!(NATURAL_VOICES.iter().all(|v| !TTS_VOICES.contains(v)));
    }
}

fn normalize_speech(text: &str) -> String {
    text.split_whitespace()
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
fn pcm_wav(pcm: &[u8]) -> Vec<u8> {
    let mut wav = Vec::with_capacity(44 + pcm.len());
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + pcm.len() as u32).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&24000u32.to_le_bytes());
    wav.extend_from_slice(&48000u32.to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
    wav.extend_from_slice(pcm);
    wav
}
