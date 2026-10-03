//! Explicit provider smoke: cargo run -p ring-providers --example smoke -- live|tts|stt
//! For an existing private bucket: add --features s3 and select s3.
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
    match std::env::args().nth(1).as_deref() {
        Some("live") => {
            let wav = OpenAi::from_env()?
                .synthesize_natural_speech("Hello. This is Silicon Ring.", "gleam")
                .await?;
            println!(
                "{{\"live\":\"ok\",\"voice\":\"gleam\",\"wav_bytes\":{}}}",
                wav.len()
            );
        }
        Some("tts") => {
            let wav = OpenAi::from_env()?
                .tts("Hello. This is Silicon Ring.", "marin")
                .await?;
            println!(
                "{{\"tts\":\"ok\",\"voice\":\"marin\",\"wav_bytes\":{}}}",
                wav.len()
            );
        }
        Some("stt") => {
            let wav = OpenAi::from_env()?
                .tts("Hello. This is Silicon Ring.", "marin")
                .await?;
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
        #[cfg(feature = "s3")]
        Some("s3") => {
            let bucket = std::env::var("RING_S3_BUCKET").map_err(|_| {
                ring_providers::Error::new(
                    "MISSING_CONFIGURATION",
                    "RING_S3_BUCKET is required.",
                    false,
                )
            })?;
            let store = ring_providers::storage::S3::new(
                bucket,
                std::env::var("AWS_REGION").ok(),
                None,
                "verification/".into(),
            )
            .await?;
            let object = format!(
                "ring-sdk-smoke-{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            );
            let expected = b"Silicon Ring private S3 round trip".to_vec();
            store.put(&object, "text/plain", expected.clone()).await?;
            let result = store.get(&object).await;
            let cleanup = store.delete(&object).await;
            let actual = result?;
            cleanup?;
            if actual != expected {
                return Err(ring_providers::Error::new(
                    "SMOKE_FAILED",
                    "S3 round trip changed the payload.",
                    false,
                ));
            }
            println!("{{\"s3\":\"ok\",\"put_get_delete\":true}}");
        }
        _ => {
            return Err(ring_providers::Error::new(
                "INVALID_INPUT",
                "Choose live, tts, stt, or s3 (requires the s3 feature).",
                false,
            ))
        }
    }
    Ok(())
}
