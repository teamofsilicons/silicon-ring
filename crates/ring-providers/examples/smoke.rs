//! Explicit paid provider smoke: cargo run -p ring-providers --example smoke -- live|tts|stt
use ring_providers::{Deepgram, OpenAi};
#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    if let Err(error) = run().await {
        eprintln!("{}", error);
        std::process::exit(1);
    }
}
async fn run() -> ring_providers::Result<()> {
    let openai = OpenAi::from_env()?;
    match std::env::args().nth(1).as_deref() {
        Some("live") => {
            let wav = openai
                .synthesize_natural_speech("Hello. This is Silicon Ring.", "gleam")
                .await?;
            println!(
                "{{\"live\":\"ok\",\"voice\":\"gleam\",\"wav_bytes\":{}}}",
                wav.len()
            );
        }
        Some("tts") => {
            let wav = openai.tts("Hello. This is Silicon Ring.", "marin").await?;
            println!(
                "{{\"tts\":\"ok\",\"voice\":\"marin\",\"wav_bytes\":{}}}",
                wav.len()
            );
        }
        Some("stt") => {
            let wav = openai.tts("Hello. This is Silicon Ring.", "marin").await?;
            let result = Deepgram::from_env()?.transcribe(wav, "audio/wav").await?;
            let transcript = result["results"]["channels"][0]["alternatives"][0]["transcript"]
                .as_str()
                .unwrap_or("");
            if !transcript.to_lowercase().contains("silicon") {
                return Err(ring_providers::Error::new(
                    "SMOKE_FAILED",
                    "Deepgram did not transcribe the expected phrase.",
                    false,
                ));
            }
            println!("{{\"stt\":\"ok\",\"model\":\"nova-3\",\"language\":\"multi\"}}");
        }
        _ => {
            return Err(ring_providers::Error::new(
                "INVALID_INPUT",
                "Choose live, tts or stt.",
                false,
            ))
        }
    }
    Ok(())
}
