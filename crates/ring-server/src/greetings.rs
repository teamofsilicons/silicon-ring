use crate::{model::*, App};
use serde_json::{json, Value};
/// Render the reviewed greeting before clients beep and open the private microphone.
pub async fn prepare(app: &App, i: &Identity, result: &mut Value) -> Result<()> {
    let vid = required(result, "voicemail_id")?.to_string();
    let (vm, existing) = {
        let e = app.engine.lock().unwrap();
        (
            e.state
                .voicemails
                .get(&vid)
                .cloned()
                .ok_or_else(|| missing("Voicemail draft"))?,
            e.state.greetings.get(&vid).cloned(),
        )
    };
    if let Some(aid) = existing {
        result["greeting"]["asset_id"] = json!(aid);
        result["greeting_asset_id"] = json!(aid);
        return Ok(());
    }
    if let Some(aid) = result["greeting"]["asset_id"].as_str() {
        let mut e = app.engine.lock().unwrap();
        e.state.greetings.insert(vid, aid.into());
        e.persist()?;
        return Ok(());
    }
    if app.providers_disabled {
        return Ok(());
    }
    let text = required(&result["greeting"], "text")?.to_string();
    let voice = {
        let e = app.engine.lock().unwrap();
        e.state
            .profiles
            .get(&key(&vm.realm, &vm.org_id, &vm.recipient))
            .map(|p| p.voice_id.clone())
            .unwrap_or_else(|| "gleam".into())
    };
    let cache = format!(
        "{}|{}|{}",
        key(&vm.realm, &vm.org_id, &vm.recipient),
        voice,
        digest(&text)
    );
    let cached = app
        .engine
        .lock()
        .unwrap()
        .state
        .greeting_cache
        .get(&cache)
        .cloned();
    let aid = if let Some(aid) = cached {
        aid
    } else {
        let provider = crate::settings::openai(app, &vm.realm, &vm.org_id, "providers.tts")
            .map_err(crate::provider_error)?;
        let bytes = synthesize(&provider, &text, &voice)
            .await
            .map_err(crate::provider_error)?;
        let mut e = app.engine.lock().unwrap();
        let aid = id("greeting");
        let path = e.data_dir.join("assets").join(&aid);
        std::fs::write(&path, &bytes).map_err(crate::auth::storage_error)?;
        e.state.assets.insert(
            aid.clone(),
            Asset {
                asset_id: aid.clone(),
                owner: key(&vm.realm, &vm.org_id, &vm.recipient),
                purpose: "voicemail_greeting".into(),
                mime_type: "audio/wav".into(),
                size_bytes: bytes.len(),
                received_bytes: bytes.len(),
                next_seq: 0,
                complete: true,
                path: path.to_string_lossy().into(),
                ringid: None,
                voicemail_id: None,
            },
        );
        e.state.greeting_cache.insert(cache, aid.clone());
        e.persist()?;
        aid
    };
    let mut e = app.engine.lock().unwrap();
    if vm.sender != i.actor {
        return Err(forbidden());
    }
    e.state.greetings.insert(vid, aid.clone());
    e.persist()?;
    result["greeting"]["asset_id"] = json!(aid);
    result["greeting_asset_id"] = json!(aid);
    Ok(())
}

pub async fn synthesize(
    provider: &ring_providers::OpenAi,
    text: &str,
    voice: &str,
) -> std::result::Result<Vec<u8>, ring_providers::Error> {
    let mut rest = text;
    let mut pcm = Vec::new();
    while !rest.is_empty() {
        let boundary = rest
            .char_indices()
            .nth(400)
            .map(|(i, _)| i)
            .unwrap_or(rest.len());
        let end = if boundary < rest.len() {
            rest[..boundary]
                .rfind(char::is_whitespace)
                .filter(|n| *n > 0)
                .unwrap_or(boundary)
        } else {
            boundary
        };
        let (chunk, next) = rest.split_at(end);
        if !chunk.trim().is_empty() {
            let wav = provider
                .synthesize_natural_speech(chunk.trim(), voice)
                .await?;
            if wav.len() < 44 || &wav[..4] != b"RIFF" {
                return Err(ring_providers::Error::new(
                    "INVALID_SYNTHESIS_AUDIO",
                    "Voice synthesis returned an invalid WAV.",
                    false,
                ));
            }
            pcm.extend_from_slice(&wav[44..]);
        }
        rest = next.trim_start();
    }
    let mut bytes = crate::media::wav_header(pcm.len() as u32).to_vec();
    bytes.extend(pcm);
    Ok(bytes)
}
