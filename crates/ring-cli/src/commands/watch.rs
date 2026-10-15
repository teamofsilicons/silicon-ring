//! `ring call watch` and `--live`: the transcript so far, then live entries and events until the
//! call ends or Ctrl-C. Watching never changes the call.

use std::collections::HashSet;

use ring_core::events::Update;
use ring_proto::{Call, CallState, EntryKind, Event, TranscriptEntry};
use serde_json::json;

use super::{Ctx, SignedIn};
use crate::error::{CliError, CliResult, SIGN_IN};
use crate::output;

/// `ring call watch`: prints a snapshot of the call first.
pub async fn watch(ctx: &Ctx, signed: &SignedIn, ringid: &str) -> CliResult<()> {
    stream(ctx, signed, ringid, true).await
}

/// `--live` after init or accept, which already printed the call.
pub async fn follow(ctx: &Ctx, signed: &SignedIn, ringid: &str) -> CliResult<()> {
    stream(ctx, signed, ringid, false).await
}

struct Watcher<'a> {
    ringid: &'a str,
    me: String,
    json: bool,
    last_seq: u64,
    state: CallState,
    delegations: HashSet<String>,
    printed_end: bool,
}

async fn stream(ctx: &Ctx, signed: &SignedIn, ringid: &str, snapshot: bool) -> CliResult<()> {
    // Subscribe before reading, so nothing between the read and the live stream is lost; every
    // (re)connect backfills by sequence number and duplicates are skipped.
    let mut subscription = signed.client.subscribe().await?;
    let call = signed.client.call(ringid).await?;
    let mut watcher = Watcher {
        ringid,
        me: signed.session.account.uuid.clone(),
        json: ctx.json,
        last_seq: 0,
        state: call.state,
        delegations: HashSet::new(),
        printed_end: false,
    };
    if snapshot {
        if watcher.json {
            output::event_line("call.snapshot", &json!({ "call": call }));
        } else {
            output::line(&format!(
                "{}  {} with {}",
                ringid,
                output::call_state(&call),
                output::others(&call, &watcher.me)
            ));
        }
    }
    watcher.backfill(signed).await?;
    if call.state == CallState::Ended {
        return watcher.end(signed, &call).await;
    }
    if !watcher.json {
        output::note("Watching. Ctrl-C stops watching; it does not hang up.");
    }

    let interrupted = tokio::signal::ctrl_c();
    tokio::pin!(interrupted);
    loop {
        let update = tokio::select! {
            _ = &mut interrupted => {
                return Err(CliError::new("interrupted", "Stopped watching. The call continues.")
                    .hint(format!("Watch again: ring call watch {ringid}   Hang up: ring call cut {ringid}")));
            }
            update = subscription.next() => update,
        };
        match update {
            None => {
                return Err(CliError::new("network", "The event stream closed.").retryable(true));
            }
            Some(Update::Connected) => {
                // Catch up on what happened while disconnected. A failed catch-up is not fatal:
                // live events keep flowing and the next reconnect tries again.
                let caught_up = async {
                    watcher.backfill(signed).await?;
                    Ok::<_, CliError>(signed.client.call(ringid).await?)
                }
                .await;
                match caught_up {
                    Ok(call) if watcher.observe_state(&call) => {
                        return watcher.end(signed, &call).await;
                    }
                    Ok(_) => {}
                    // With --json, stderr carries only the final error envelope.
                    Err(e) if !watcher.json => output::note(&format!(
                        "Could not catch up after reconnecting: {}",
                        e.message
                    )),
                    Err(_) => {}
                }
            }
            Some(Update::Disconnected {
                reason,
                retry_in_ms,
            }) => {
                if !watcher.json {
                    output::note(&format!(
                        "Connection lost ({reason}); reconnecting in {:.0} s.",
                        retry_in_ms as f64 / 1000.0
                    ));
                }
            }
            Some(Update::SignedOut) => {
                return Err(
                    CliError::new("session_expired", "The session ended while watching.")
                        .hint(format!("Sign in again: {SIGN_IN}")),
                );
            }
            Some(Update::Event(event)) => {
                if event.ringid() == Some(ringid) && watcher.event(signed, event).await? {
                    return Ok(());
                }
            }
        }
    }
}

impl Watcher<'_> {
    /// Prints every entry after the last one shown, following `next_after_seq` page by page.
    async fn backfill(&mut self, signed: &SignedIn) -> CliResult<()> {
        let mut cursor = self.last_seq;
        loop {
            let page = signed
                .client
                .transcript(self.ringid, cursor, Some(200))
                .await?;
            let count = page.items.len();
            for entry in page.items {
                self.entry(entry);
            }
            let next = page.next_after_seq.max(self.last_seq);
            if count < 200 || next <= cursor {
                return Ok(());
            }
            cursor = next;
        }
    }

    fn entry(&mut self, entry: TranscriptEntry) {
        if entry.seq <= self.last_seq {
            return;
        }
        self.last_seq = self.last_seq.max(entry.seq);
        if entry.kind == EntryKind::CallEnded {
            self.printed_end = true;
        }
        if self.json {
            output::json(&Event::TranscriptEntry {
                ringid: self.ringid.to_owned(),
                entry,
            });
            return;
        }
        output::line(&output::entry(&entry));
        if entry.kind == EntryKind::Delegation
            && let Some(id) = entry.data.get("delegation_id").and_then(|v| v.as_str())
        {
            self.delegations.insert(id.to_owned());
            output::line(&format!(
                "  answer: ring send {} commentary \"…\" --delegation-id {id}",
                self.ringid
            ));
        }
    }

    /// Handles one live event for this call. Returns true when the call is over.
    async fn event(&mut self, signed: &SignedIn, event: Event) -> CliResult<bool> {
        match event {
            Event::TranscriptEntry { entry, .. } => {
                self.entry(entry);
                Ok(false)
            }
            Event::TranscriptPartial { .. } => Ok(false),
            Event::CallEnded { call } => {
                self.end(signed, &call).await?;
                Ok(true)
            }
            Event::CallUpdated { call } => {
                if self.json {
                    output::json(&Event::CallUpdated { call: call.clone() });
                }
                if self.observe_state(&call) {
                    self.end(signed, &call).await?;
                    return Ok(true);
                }
                Ok(false)
            }
            Event::DelegationCreated { delegation } => {
                if self.json {
                    output::json(&Event::DelegationCreated { delegation });
                } else if self.delegations.insert(delegation.delegation_id.clone()) {
                    output::line(&format!(
                        "» delegation {}: {}\n  answer: ring send {} commentary \"…\" --delegation-id {}",
                        delegation.delegation_id,
                        delegation.request.as_deref().unwrap_or("(no request text)"),
                        self.ringid,
                        delegation.delegation_id
                    ));
                }
                Ok(false)
            }
            other => {
                if self.json {
                    output::json(&other);
                } else {
                    match &other {
                        Event::CallRingingStopped { reason, .. } => {
                            output::line(&format!("· ringing stopped ({})", reason.as_str()))
                        }
                        Event::VoicemailReceived { voicemail }
                        | Event::VoicemailUpdated { voicemail } => output::line(&format!(
                            "· voicemail {}",
                            output::voicemail_row(voicemail).trim_start()
                        )),
                        _ => {}
                    }
                }
                Ok(false)
            }
        }
    }

    /// Notes a state change. Returns true when the call has ended.
    fn observe_state(&mut self, call: &Call) -> bool {
        if call.state == self.state {
            return false;
        }
        self.state = call.state;
        match call.state {
            CallState::Ended => return true,
            CallState::Active if !self.json => output::line("· the call is active"),
            CallState::Voicemail if !self.json => {
                if call.created_by.uuid == self.me {
                    output::line(&format!(
                        "· the call went to voicemail. Leave a message: ring voicemail leave {} --text \"…\"",
                        self.ringid
                    ));
                } else {
                    output::line("· the call went to voicemail");
                }
            }
            _ => {}
        }
        false
    }

    /// Prints whatever the transcript still holds, then the end of the call.
    async fn end(&mut self, signed: &SignedIn, call: &Call) -> CliResult<()> {
        self.backfill(signed).await?;
        if self.json {
            output::json(&Event::CallEnded { call: call.clone() });
        }
        self.finish(call);
        Ok(())
    }

    fn finish(&mut self, call: &Call) {
        if self.json || self.printed_end {
            return;
        }
        self.printed_end = true;
        let length = match (call.answered_at, call.ended_at) {
            (Some(a), Some(e)) => format!(
                " after {}",
                output::duration_ms((e.millis() - a.millis()).max(0) as u64)
            ),
            _ => String::new(),
        };
        output::line(&format!(
            "· call ended ({}){length}",
            call.end_reason.map(|r| r.as_str()).unwrap_or("completed")
        ));
    }
}
