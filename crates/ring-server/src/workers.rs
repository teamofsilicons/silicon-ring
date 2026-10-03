use crate::{engine::entry_visible, model::*, App};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use futures_util::{SinkExt, StreamExt};
use ring_providers::voice;
use serde_json::{json, Value};
use std::{collections::BTreeSet, time::Duration};
use tokio::sync::mpsc;

pub fn start(app: App) {
    let mixing = app.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_millis(20));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Burst);
        loop {
            tick.tick().await;
            let mut e = mixing.engine.lock().unwrap();
            let changed = mixing.media.lock().unwrap().tick(&mut e);
            if changed {
                if let Err(error) = e.persist() {
                    tracing::error!(code=%error.code,"Audio metadata persistence failed");
                }
            }
        }
    });
    let publishing = app.clone();
    tokio::spawn(async move {
        let mut timer = tokio::time::interval(Duration::from_millis(250));
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            timer.tick().await;
            if !publishing.providers_disabled {
                publish_pending(&publishing).await;
            }
        }
    });
    tokio::spawn(async move {
        let mut timer = tokio::time::interval(Duration::from_secs(1));
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut count = 0u64;
        let mut started = BTreeSet::new();
        let mut transcription_started = BTreeSet::new();
        let mut voice_jobs = BTreeSet::new();
        let mut stt_jobs = BTreeSet::new();
        loop {
            timer.tick().await;
            count += 1;
            let (calls, voicemails) = {
                let mut e = app.engine.lock().unwrap();
                if e.tick(count % 10 == 0) {
                    if let Err(error) = e.persist() {
                        tracing::error!(code=%error.code,"Lifecycle persistence failed");
                    }
                }
                (
                    e.state
                        .calls
                        .values()
                        .filter(|c| c.state == "active")
                        .cloned()
                        .collect::<Vec<_>>(),
                    e.state
                        .voicemails
                        .values()
                        .filter(|v| v.state == "draft" || v.state == "delivered")
                        .cloned()
                        .collect::<Vec<_>>(),
                )
            };
            for c in calls {
                for p in c
                    .participants
                    .iter()
                    .filter(|p| silicon(&p.actor) && p.left_at.is_none())
                {
                    let rep = format!("{}_{}", c.ringid, p.actor);
                    if started.insert(rep.clone()) && !app.providers_disabled {
                        let app = app.clone();
                        let c = c.clone();
                        let p = p.clone();
                        tokio::spawn(async move {
                            let result = representative(&app, &c, &p).await;
                            if let Err(error) = result {
                                representative_failure(&app, &c, &p.actor, &error);
                            }
                        });
                    }
                }
                if !app.providers_disabled && transcription_started.insert(c.ringid.clone()) {
                    let a = app.clone();
                    tokio::spawn(async move {
                        if let Err(error) = transcriber(&a, &c).await {
                            let mut e = a.engine.lock().unwrap();
                            e.state.call_event(&c,"transcription.failed",json!({"ringid":c.ringid,"code":error.code,"message":error.message}));
                            let _ = e.persist();
                        }
                    });
                }
            }
            if !app.providers_disabled {
                for v in voicemails {
                    if v.synthesis_status == "processing"
                        && v.text.is_some()
                        && voice_jobs.insert(v.voicemail_id.clone())
                    {
                        let a = app.clone();
                        tokio::spawn(async move {
                            synthesize_voicemail(a, v).await;
                        });
                    } else if v.state == "delivered"
                        && v.format == "audio"
                        && v.complete_audio
                        && v.transcription_status == "pending"
                        && stt_jobs.insert(v.voicemail_id.clone())
                    {
                        let a = app.clone();
                        tokio::spawn(async move {
                            transcribe_voicemail(a, v).await;
                        });
                    }
                }
            }
        }
    });
}
async fn representative(
    app: &App,
    c: &Call,
    p: &Participant,
) -> std::result::Result<(), ring_providers::Error> {
    let voice = app
        .engine
        .lock()
        .unwrap()
        .state
        .profiles
        .get(&key(&c.realm, &c.org_id, &p.actor))
        .map(|v| v.voice_id.clone())
        .unwrap_or_else(|| "gleam".into());
    let provider = crate::settings::openai(app, &c.realm, &c.org_id, "providers.live")?;
    let mut socket = provider
        .connect_live(&voice, &p.context, Some(&p.start))
        .await?;
    let live_offset = c
        .answered_at
        .as_deref()
        .and_then(|v| chrono::DateTime::parse_from_rfc3339(v).ok())
        .map(|t| (chrono::Utc::now().timestamp_millis() - t.timestamp_millis()).max(0) as u64)
        .unwrap_or(0);
    let (tx, mut audio) = mpsc::channel::<Vec<u8>>(100);
    let sid = app.media.lock().unwrap().representative(c, &p.actor, tx);
    let mut check = tokio::time::interval(Duration::from_millis(200));
    let mut roster = c
        .participants
        .iter()
        .filter(|p| p.left_at.is_none())
        .map(|p| p.actor.clone())
        .collect::<BTreeSet<_>>();
    let result = async {
        loop {
            tokio::select! {
                sample = audio.recv() => {
                    let Some(bytes) = sample else { break };
                    voice::send_event(&mut socket, voice::audio_event(&bytes)?).await?;
                },
                received = voice::receive_event(&mut socket) => {
                    let event = received?;
                    match event["type"].as_str() {
                        Some("session.output_audio.delta") => {
                            if let Some(raw) = event["delta"].as_str() {
                                if let Ok(bytes) = STANDARD.decode(raw) { app.media.lock().unwrap().push_representative(&sid, &bytes); }
                            }
                        },
                        Some("session.output_transcript.delta") => {
                            let mut caption = event.clone();
                            for field in ["start_ms", "end_ms"] { if let Some(value) = caption[field].as_u64() { caption[field] = json!(value + live_offset); } }
                            add_transcript(app, &c.ringid, Some(p.actor.clone()), vec![p.actor.clone()], &caption, "live");
                        },
                        Some("session.delegation.created") if event["delegation"]["target"] == "client" => create_delegation(app, c, &p.actor, &event),
                        Some("session.thinking.appended" | "session.commentary.appended") => message_status(app, c, &p.actor, event["client_event_id"].as_str(), "accepted"),
                        Some("error") => {
                            message_status(app, c, &p.actor, event["error"]["client_event_id"].as_str(), "failed");
                            let mut e = app.engine.lock().unwrap();
                            let i = Identity { actor: p.actor.clone(), org_id: c.org_id.clone(), realm: c.realm.clone(), display_name: p.display_name.clone(), admin: false };
                            e.state.event(&i, vec![p.actor.clone()], "representative.error", json!({"ringid":c.ringid,"code":event["error"]["code"],"client_event_id":event["error"]["client_event_id"]}));
                            let _ = e.persist();
                        },
                        Some("session.closed") => return Err(ring_providers::Error::new("REPRESENTATIVE_ENDED", "The voice provider ended this representative.", true)),
                        _ => {}
                    }
                },
                _ = check.tick() => {
                    let (members, messages) = {
                        let e = app.engine.lock().unwrap();
                        let current = e.state.calls.get(&c.ringid).filter(|c| c.state == "active" && c.active(&p.actor));
                        let Some(current) = current else { break };
                        let messages = pending_messages(&e.state, current, &p.actor);
                        (current.participants.iter().filter(|p| p.left_at.is_none()).map(|p| p.actor.clone()).collect::<BTreeSet<_>>(), messages)
                    };
                    for joined in members.difference(&roster) { voice::send_event(&mut socket, voice::update_event("instructions", &format!("{joined} joined this call."), None)?).await?; }
                    for left in roster.difference(&members) { voice::send_event(&mut socket, voice::update_event("instructions", &format!("{left} left this call."), None)?).await?; }
                    roster = members;
                    for message in messages {
                        let mut event = voice::update_event(message["kind"].as_str().unwrap_or("thinking"), message["text"].as_str().unwrap_or(""), message["delegation_id"].as_str())?;
                        event["event_id"] = message["message_id"].clone();
                        voice::send_event(&mut socket, event).await?;
                        message_status(app, c, &p.actor, message["message_id"].as_str(), "sent");
                    }
                }
            }
        }
        Ok(())
    }.await;
    app.media.lock().unwrap().streams.remove(&sid);
    let finalized = voice::close_live(&mut socket).await;
    result?;
    finalized?;
    Ok(())
}
fn pending_messages(state: &State, call: &Call, actor: &str) -> Vec<Value> {
    call.transcript
        .iter()
        .filter(|entry| {
            entry.actor.as_deref() == Some(actor)
                && matches!(entry.kind.as_str(), "thinking" | "commentary")
        })
        .filter(|entry| {
            entry.data["message_id"]
                .as_str()
                .is_some_and(|id| !state.sent_messages.contains_key(id))
        })
        .map(|entry| entry.data.clone())
        .collect()
}
fn message_status(app: &App, c: &Call, actor: &str, message_id: Option<&str>, status: &str) {
    let Some(message_id) = message_id else { return };
    let mut e = app.engine.lock().unwrap();
    let Some(call) = e.state.calls.get(&c.ringid) else {
        return;
    };
    if !call
        .transcript
        .iter()
        .any(|t| t.actor.as_deref() == Some(actor) && t.data["message_id"] == message_id)
    {
        return;
    }
    if e.state
        .sent_messages
        .get(message_id)
        .is_some_and(|v| v == status)
    {
        return;
    }
    e.state
        .sent_messages
        .insert(message_id.into(), status.into());
    if status != "sent" {
        let i = Identity {
            actor: actor.into(),
            org_id: c.org_id.clone(),
            realm: c.realm.clone(),
            display_name: actor.into(),
            admin: false,
        };
        e.state.event(
            &i,
            vec![actor.into()],
            "representative.delivery",
            json!({"ringid":c.ringid,"message_id":message_id,"status":status}),
        );
    }
    if let Err(error) = e.persist() {
        tracing::error!(code=%error.code, "Representative delivery status persistence failed");
    }
}
fn representative_failure(app: &App, c: &Call, actor: &str, error: &ring_providers::Error) {
    let mut e = app.engine.lock().unwrap();
    if let Some(call) = e.state.calls.get_mut(&c.ringid) {
        call.entry(
            "representative.failed",
            Some(actor.into()),
            json!({"code":error.code,"message":error.message}),
            None,
        );
    }
    e.state.call_event(
        c,
        "representative.failed",
        json!({"ringid":c.ringid,"actor":actor,"code":error.code,"message":error.message}),
    );
    let _ = e.persist();
    app.media
        .lock()
        .unwrap()
        .streams
        .remove(&format!("rep_{}_{}", c.ringid, actor));
}
pub fn add_transcript(
    app: &App,
    ring: &str,
    actor: Option<String>,
    speakers: Vec<String>,
    event: &Value,
    source: &str,
) {
    let Some(text) = event["delta"].as_str().filter(|t| !t.is_empty()) else {
        return;
    };
    let mut e = app.engine.lock().unwrap();
    let Some(call) = e.state.calls.get_mut(ring) else {
        return;
    };
    let entry=call.entry("speech",actor,json!({"text":text,"delta":text,"speaker_ids":speakers,"start_ms":event["start_ms"],"end_ms":event["end_ms"],"segment_id":event["segment_id"],"revision":event["revision"],"source":source,"final":event["final"]}),None);
    let call = call.clone();
    let recipients = call
        .participants
        .iter()
        .filter(|p| entry_visible(&call, &entry, &p.actor))
        .map(|p| p.actor.clone())
        .collect();
    let i = Identity {
        actor: call.caller.clone(),
        org_id: call.org_id.clone(),
        realm: call.realm.clone(),
        display_name: String::new(),
        admin: false,
    };
    // Speech is batched for Ting every ten seconds; clients still see each delta immediately.
    e.state.event(
        &i,
        recipients,
        "transcript.delta",
        json!({"ringid":ring,"entry":entry}),
    );
    let _ = e.persist();
}
fn create_delegation(app: &App, c: &Call, actor: &str, event: &Value) {
    let Some(did) = event["delegation"]["id"].as_str() else {
        return;
    };
    let mut e = app.engine.lock().unwrap();
    if e.state.delegations.contains_key(did) {
        return;
    }
    let Some(call) = e.state.calls.get(&c.ringid) else {
        return;
    };
    let threshold = after(-30);
    let context: Vec<_> = call
        .transcript
        .iter()
        .filter(|t| t.occurred_at >= threshold && entry_visible(call, t, actor))
        .cloned()
        .collect();
    let d=Delegation{delegation_id:did.into(),ringid:c.ringid.clone(),actor:actor.into(),org_id:c.org_id.clone(),realm:c.realm.clone(),request:"The representative requested backend assistance. Determine the task from the attached recent transcript; the provider does not supply separate request text.".into(),context,responses:vec![],status:"open".into(),created_at:now()};
    e.state.delegations.insert(did.into(), d.clone());
    if let Some(call) = e.state.calls.get_mut(&c.ringid) {
        call.entry(
            "delegation.created",
            Some(actor.into()),
            json!({"delegation_id":did,"offset_ms":event["offset_ms"]}),
            Some(actor.into()),
        );
    }
    let i = Identity {
        actor: actor.into(),
        org_id: c.org_id.clone(),
        realm: c.realm.clone(),
        display_name: actor.into(),
        admin: false,
    };
    e.state
        .event(&i, vec![actor.into()], "delegation.created", json!(d));
    let _ = e.persist();
}
async fn synthesize_voicemail(app: App, vm: Voicemail) {
    let voice = {
        let e = app.engine.lock().unwrap();
        e.state
            .profiles
            .get(&key(&vm.realm, &vm.org_id, &vm.sender))
            .map(|p| p.voice_id.clone())
            .unwrap_or_else(|| "gleam".into())
    };
    let result = match crate::settings::openai(&app, &vm.realm, &vm.org_id, "providers.tts") {
        Ok(p) => crate::greetings::synthesize(&p, vm.text.as_deref().unwrap_or(""), &voice).await,
        Err(e) => Err(e),
    };
    let mut e = app.engine.lock().unwrap();
    match result {
        Ok(bytes) => {
            let aid = id("asset");
            let path = e.data_dir.join("assets").join(&aid);
            let saved = std::fs::write(&path, &bytes).is_ok();
            if saved {
                e.state.assets.insert(
                    aid.clone(),
                    Asset {
                        asset_id: aid.clone(),
                        owner: key(&vm.realm, &vm.org_id, &vm.sender),
                        purpose: "voicemail".into(),
                        mime_type: "audio/wav".into(),
                        size_bytes: bytes.len(),
                        received_bytes: bytes.len(),
                        next_seq: 0,
                        complete: true,
                        path: path.to_string_lossy().into(),
                        ringid: None,
                        voicemail_id: Some(vm.voicemail_id.clone()),
                    },
                );
            }
            if let Some(v) = e.state.voicemails.get_mut(&vm.voicemail_id) {
                v.synthesis_status = if saved { "ready" } else { "failed" }.into();
                v.complete_audio = saved;
                v.audio_asset_id = if saved { Some(aid) } else { None };
                if !saved {
                    v.error = Some("Audio could not be persisted".into());
                }
            }
        }
        Err(error) => {
            if let Some(v) = e.state.voicemails.get_mut(&vm.voicemail_id) {
                v.synthesis_status = "failed".into();
                v.error = Some(format!("{}: {}", error.code, error.message));
            }
        }
    }
    let i = Identity {
        actor: vm.sender.clone(),
        org_id: vm.org_id,
        realm: vm.realm,
        display_name: vm.sender.clone(),
        admin: false,
    };
    e.state.event(
        &i,
        vec![vm.sender],
        "voicemail.updated",
        json!({"voicemail_id":vm.voicemail_id}),
    );
    let _ = e.persist();
}
async fn transcribe_voicemail(app: App, vm: Voicemail) {
    let bytes = {
        let e = app.engine.lock().unwrap();
        vm.audio_asset_id
            .as_ref()
            .and_then(|a| e.state.assets.get(a))
            .and_then(|a| std::fs::read(&a.path).ok())
    };
    let result = match (
        crate::settings::deepgram(&app, &vm.realm, &vm.org_id),
        bytes,
    ) {
        (Ok(p), Some(bytes)) => p.transcribe(bytes, "audio/wav").await,
        (Err(e), _) => Err(e),
        _ => Err(ring_providers::Error::new(
            "AUDIO_MISSING",
            "Recorded audio could not be read.",
            false,
        )),
    };
    let mut e = app.engine.lock().unwrap();
    if let Some(v) = e.state.voicemails.get_mut(&vm.voicemail_id) {
        match result {
            Ok(text) => {
                v.transcript = text["results"]["channels"][0]["alternatives"][0]["transcript"]
                    .as_str()
                    .map(String::from);
                v.transcription_status = if v.transcript.is_some() {
                    "ready"
                } else {
                    "failed"
                }
                .into();
            }
            Err(error) => {
                v.transcription_status = "failed".into();
                v.error = Some(format!("{}: {}", error.code, error.message));
            }
        }
    }
    let i = Identity {
        actor: vm.sender,
        org_id: vm.org_id,
        realm: vm.realm,
        display_name: String::new(),
        admin: false,
    };
    let transcript = e
        .state
        .voicemails
        .get(&vm.voicemail_id)
        .and_then(|v| v.transcript.clone());
    e.state.event(
        &i,
        vec![vm.recipient],
        "voicemail.updated",
        json!({"voicemail_id":vm.voicemail_id,"transcript":transcript}),
    );
    let _ = e.persist();
}
async fn publish_pending(app: &App) {
    let entries = {
        let mut e = app.engine.lock().unwrap();
        let timestamp = now();
        let mut ready = e
            .state
            .publications
            .values_mut()
            .filter(|n| {
                n.status == "pending"
                    || n.status == "failed" && n.retry_at.as_ref().is_some_and(|t| t <= &timestamp)
            })
            .collect::<Vec<_>>();
        ready.sort_by_key(|n| match n.event_type.as_str() {
            "delegation.created" => 0,
            "call.incoming" => 1,
            _ => 2,
        });
        let rows = ready
            .into_iter()
            .take(20)
            .map(|n| {
                n.status = "sending".into();
                n.attempts += 1;
                n.clone()
            })
            .collect::<Vec<_>>();
        if rows.is_empty() {
            return;
        }
        if let Err(error) = e.persist() {
            for row in rows {
                if let Some(n) = e.state.publications.get_mut(&row.notification_id) {
                    n.status = "pending".into();
                    n.attempts = n.attempts.saturating_sub(1);
                }
            }
            tracing::error!(code=%error.code, "Notification outbox persistence failed");
            return;
        }
        rows
    };
    for entry in entries {
        let result = if entry.realm == "test" && !app.test_tokens.is_empty() {
            Err(Fault::new(
                "TEST_DELIVERY_ISOLATED",
                "Test publications are retained locally and never sent to production Ting.",
                "Inspect the isolated test outbox.",
            ))
        } else {
            match crate::auth::ting_token(app, &entry).await {
                Ok(token) => match ring_providers::Ting::for_realm(entry.realm == "test") {
                    Ok(ting) => match ring_providers::Ting::prepare(
                        &entry.org_id,
                        &entry.actor,
                        &format!("ring.{}", entry.event_type),
                        &entry.notification_id,
                        entry.data.clone(),
                    ) {
                        Ok(bytes) => ting
                            .publish(&token, &bytes)
                            .await
                            .map(|_| ())
                            .map_err(crate::provider_error),
                        Err(e) => Err(crate::provider_error(e)),
                    },
                    Err(e) => Err(crate::provider_error(e)),
                },
                Err(e) => Err(e),
            }
        };
        let mut e = app.engine.lock().unwrap();
        if let Some(n) = e.state.publications.get_mut(&entry.notification_id) {
            match result {
                Ok::<(), Fault>(()) => {
                    n.status = "delivered".into();
                    n.error = None;
                    n.retry_at = None;
                }
                Err(error) => {
                    n.status = "failed".into();
                    n.retry_at = error.retryable.then(|| after(2i64.pow(n.attempts.min(8))));
                    n.error = Some(format!("{}: {}", error.code, error.message));
                }
            }
        }
        let _ = e.persist();
    }
}
async fn transcriber(app: &App, call: &Call) -> std::result::Result<(), ring_providers::Error> {
    let provider = crate::settings::deepgram(app, &call.realm, &call.org_id)?;
    let mut socket = provider.connect_stream().await?;
    let (tx, mut audio) = mpsc::channel::<Vec<u8>>(100);
    let call_start = call
        .answered_at
        .as_deref()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|t| t.timestamp_millis())
        .unwrap_or_else(|| chrono::Utc::now().timestamp_millis());
    let offset = (chrono::Utc::now().timestamp_millis() - call_start).max(0) as u64;
    app.media
        .lock()
        .unwrap()
        .transcribers
        .insert(call.ringid.clone(), tx);
    let mut check = tokio::time::interval(Duration::from_millis(100));
    let mut revision = 0u64;
    let mut closing = None;
    let result = loop {
        tokio::select! {
            bytes = audio.recv(), if closing.is_none() => {
                if let Some(bytes) = bytes {
                    if socket.send(tokio_tungstenite::tungstenite::Message::Binary(bytes.into())).await.is_err() { break Err(transcription_lost()); }
                }
            },
            message = socket.next() => match message {
                Some(Ok(tokio_tungstenite::tungstenite::Message::Text(raw))) => {
                    if let Ok(event) = serde_json::from_str::<Value>(&raw) {
                        if event["type"] == "Results" { transcription_result(app, call, &event, offset, &mut revision); }
                        else if event["type"] == "Metadata" && closing.is_some() { break Ok(()); }
                        else if event["type"] == "Error" { break Err(transcription_lost()); }
                    }
                },
                Some(Ok(tokio_tungstenite::tungstenite::Message::Ping(data))) => { let _ = socket.send(tokio_tungstenite::tungstenite::Message::Pong(data)).await; },
                Some(Ok(tokio_tungstenite::tungstenite::Message::Close(_))) | None => { break if closing.is_some() { Ok(()) } else { Err(transcription_lost()) }; },
                Some(Err(_)) => break Err(transcription_lost()),
                _ => {}
            },
            _ = check.tick() => {
                if closing.is_some_and(|deadline| tokio::time::Instant::now() >= deadline) {
                    break Err(ring_providers::Error::new("TRANSCRIPTION_FINALIZATION_UNCONFIRMED", "The transcription provider did not confirm final captions within five seconds.", true));
                }
                let active = app.engine.lock().unwrap().state.calls.get(&call.ringid).is_some_and(|c| c.state == "active");
                if !active && closing.is_none() {
                    // Drain the final Results instead of discarding the last spoken sentence.
                    if socket.send(tokio_tungstenite::tungstenite::Message::Text(json!({"type":"CloseStream"}).to_string().into())).await.is_err() { break Err(transcription_lost()); }
                    closing = Some(tokio::time::Instant::now() + Duration::from_secs(5));
                }
            }
        }
    };
    app.media.lock().unwrap().transcribers.remove(&call.ringid);
    let _ = socket.close(None).await;
    result
}
fn transcription_lost() -> ring_providers::Error {
    ring_providers::Error::new(
        "TRANSCRIPTION_CONNECTION_LOST",
        "Deepgram streaming connection was interrupted.",
        true,
    )
}
fn transcription_result(app: &App, call: &Call, event: &Value, offset: u64, revision: &mut u64) {
    let text = event["channel"]["alternatives"][0]["transcript"]
        .as_str()
        .unwrap_or("");
    if text.is_empty() {
        return;
    }
    let start = (event["start"].as_f64().unwrap_or(0.0) * 1000.0) as u64 + offset;
    let end = start + (event["duration"].as_f64().unwrap_or(0.0) * 1000.0) as u64;
    let mut speakers = app.media.lock().unwrap().speakers(&call.ringid, start, end);
    if speakers.is_empty() {
        let e = app.engine.lock().unwrap();
        if let Some(current) = e.state.calls.get(&call.ringid) {
            let carbons = current
                .participants
                .iter()
                .filter(|p| !silicon(&p.actor))
                .map(|p| p.actor.clone())
                .collect::<BTreeSet<_>>();
            if carbons.len() == 1 {
                speakers = carbons.into_iter().collect();
            }
        }
    }
    *revision += 1;
    add_transcript(
        app,
        &call.ringid,
        if speakers.len() == 1 {
            Some(speakers[0].clone())
        } else {
            None
        },
        speakers,
        &json!({"delta":text,"start_ms":start,"end_ms":end,"segment_id":format!("stt_{start}"),"revision":revision,"final":event["is_final"]}),
        "deepgram",
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Engine;

    #[test]
    fn pending_representative_messages_survive_startup_and_exact_retries() {
        let dir = tempfile::tempdir().unwrap();
        let mut e = Engine::open(dir.path()).unwrap();
        let mut login = |actor: &str| {
            let value = e
                .login(
                    Identity {
                        actor: actor.into(),
                        org_id: "org".into(),
                        realm: "test".into(),
                        display_name: actor.into(),
                        admin: false,
                    },
                    None,
                )
                .unwrap();
            e.session(value["session_token"].as_str().unwrap()).unwrap()
        };
        let a = login("si:alice");
        let b = login("si:bob");
        let c = e
            .dispatch(&a, "calls.init", &json!({"target":"si:bob"}))
            .unwrap();
        let ring = c["ringid"].as_str().unwrap();
        e.dispatch(&b, "calls.accept", &json!({"ringid":ring}))
            .unwrap();
        let params =
            json!({"ringid":ring,"kind":"thinking","text":"I am checking.","delegation_id":null});
        let sent = e.request(&a, "same-request", "representative.send", params.clone());
        assert_eq!(sent["ok"], true);
        assert_eq!(
            e.request(&a, "same-request", "representative.send", params),
            sent
        );
        assert_eq!(
            pending_messages(&e.state, &e.state.calls[ring], "si:alice").len(),
            1
        );
        assert!(pending_messages(&e.state, &e.state.calls[ring], "si:bob").is_empty());
        let message_id = sent["result"]["message_id"].as_str().unwrap();
        e.state
            .sent_messages
            .insert(message_id.into(), "accepted".into());
        e.persist().unwrap();
        drop(e);
        let reopened = Engine::open(dir.path()).unwrap();
        assert!(
            pending_messages(&reopened.state, &reopened.state.calls[ring], "si:alice").is_empty()
        );
    }
}
