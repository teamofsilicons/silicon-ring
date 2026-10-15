//! A silicon's voice representative: a GPT-Live session bridged into the call's LiveKit room.
//! It hears everyone else (mix-minus, authoritative devices only), speaks with the silicon's
//! Natural Voice, captions its own speech and relays delegations and the silicon's replies.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Result, anyhow, bail};
use ring_proto::{
    Account, AccountKind, RepresentativeStatus,
    internal::{FromMedia, RosterMember},
};

use crate::{
    audio::{self, Buffering, Clock, Feeds, LIVE_RATE},
    link::Reporter,
    live::{self, Event, Live, SILENCE_LIMIT},
    room::{self, Change, Voice},
    roster::{Roster, describe},
    stt::new_segment_id,
    supervisor::{RepControl, Session, Supervisor},
};
use tokio_util::sync::CancellationToken;

pub struct Params {
    pub ringid: String,
    pub silicon: Account,
    pub voice: String,
    pub context: String,
    pub start: Option<String>,
    pub roster: Vec<RosterMember>,
    pub token: String,
}

/// Bounded playout backlog; GPT-Live streams in real time, so more means we fell behind.
const MAX_VOICE_BACKLOG: Duration = Duration::from_secs(2);
/// Wait this long after a delegation for the last words that prompted it.
const DELEGATION_GRACE: Duration = Duration::from_millis(600);
/// The longest the start text waits for someone else in the room to hear it.
const START_WAIT: Duration = Duration::from_secs(10);
/// Waits before each attempt to replace a failed GPT-Live session.
const RESUME_DELAYS: [Duration; 3] = [
    Duration::ZERO,
    Duration::from_secs(1),
    Duration::from_secs(3),
];
/// A session that fails again this soon after being replaced is not retried.
const MIN_RESUME_INTERVAL: Duration = Duration::from_secs(30);

pub async fn run(supervisor: Arc<Supervisor>, params: Params, mut session: Session<RepControl>) {
    let report = |status, error| {
        supervisor.reporter.send(FromMedia::RepresentativeStatus {
            ringid: params.ringid.clone(),
            silicon_uuid: params.silicon.uuid.clone(),
            status,
            error,
        })
    };
    report(RepresentativeStatus::Starting, None);
    tracing::info!(ringid = %params.ringid, silicon = %params.silicon.id, "representative starting");
    match serve(&supervisor, &params, &mut session).await {
        // ring-server starts a representative again when the worker reconnects, as long as it
        // still counts it as running, so a worker restart must not end it for the whole call.
        Ok(()) if supervisor.shutdown.is_cancelled() => {
            tracing::info!(ringid = %params.ringid, silicon = %params.silicon.id, "representative left the call for a worker restart");
            return;
        }
        Ok(()) => report(RepresentativeStatus::Ended, None),
        Err(e) => {
            tracing::warn!(ringid = %params.ringid, silicon = %params.silicon.id, "representative failed: {e:#}");
            report(RepresentativeStatus::Failed, Some(format!("{e:#}")));
        }
    }
    tracing::info!(ringid = %params.ringid, silicon = %params.silicon.id, "representative ended");
}

async fn serve(
    supervisor: &Supervisor,
    p: &Params,
    session: &mut Session<RepControl>,
) -> Result<()> {
    let api_key = supervisor.config.openai_key()?.to_owned();
    let roster = Roster::new(p.roster.clone());
    let instructions = instructions(&p.silicon, &p.context, &roster);
    // Not cancellable: a half-finished join would leave a participant behind in the room.
    let (live, joined) = tokio::join!(
        Live::connect(&api_key, &p.voice, &instructions),
        room::join(&supervisor.config.livekit_url, &p.token),
    );
    let (mut live, (room, events)) = match (live, joined) {
        (Ok(live), Ok(joined)) => (live, joined),
        (Err(e), joined) => {
            if let Ok((room, _)) = joined {
                let _ = room.close().await;
            }
            return Err(e);
        }
        (Ok(live), Err(e)) => {
            live.close().await;
            return Err(e);
        }
    };
    if session.stop.is_cancelled() {
        live.close().await;
        let _ = room.close().await;
        return Ok(());
    }
    let joined_at = Instant::now();
    let mut bridge = Bridge {
        reporter: supervisor.reporter.clone(),
        ringid: p.ringid.clone(),
        silicon: p.silicon.clone(),
        api_key,
        voice: p.voice.clone(),
        context: p.context.clone(),
        resumed_at: None,
        roster,
        captions: Captions::new(
            supervisor.reporter.clone(),
            p.ringid.clone(),
            p.silicon.uuid.clone(),
            joined_at,
        ),
        heard: Heard::default(),
        delegations: Vec::new(),
        delegation_ids: HashMap::new(),
        awaiting: HashMap::new(),
        delivered: HashSet::new(),
    };
    let result = bridge
        .run(&mut live, &room, events, session, p.start.as_deref())
        .await;
    bridge.captions.finish();
    live.close().await;
    let _ = room.close().await;
    result
}

/// Milliseconds from `since` to `at`, negative when `at` came first.
fn signed_ms(at: Instant, since: Instant) -> i64 {
    match at.checked_duration_since(since) {
        Some(d) => d.as_millis() as i64,
        None => -(since.duration_since(at).as_millis() as i64),
    }
}

fn instructions(silicon: &Account, context: &str, roster: &Roster) -> String {
    let name = silicon.label();
    let me = describe(silicon.display_name.as_deref(), &silicon.id);
    let mut text = format!(
        "You are the voice of {me} on a live Silicon Ring voice call. {name} is a silicon, an AI agent \
         that cannot speak on calls itself, so you speak for it, in the first person, as {name}. Talk like a \
         warm, natural person on a phone call: short turns, plain spoken words, no lists or formatting, and \
         let people finish.\n\n\
         You are {name}: say \"I\" and \"me\", never talk about {name} in the third person, and never \
         say you are checking with {name} or with anyone else. You only know what is said on this call \
         and what {name} tells you. Whenever something needs {name}'s knowledge, tools, memory, decisions \
         or commitments, say a few words such as \"let me check\" or \"one moment\", then delegate it to \
         {name}. Never guess or invent facts, results or promises. {name}'s replies reach you as thinking \
         (private background; never read it out) or commentary (say it to the others right away, in your \
         own words, as soon as there is a natural pause).\n\n"
    );
    let others: Vec<String> = roster
        .members()
        .filter(|m| m.uuid != silicon.uuid)
        .map(|m| {
            let kind = match m.kind {
                AccountKind::Carbon => "a person",
                AccountKind::Silicon => "a silicon",
            };
            format!("{}, {kind}", describe(m.display_name.as_deref(), &m.id))
        })
        .collect();
    if others.is_empty() {
        text.push_str(
            "Nobody else is on the call yet. You will be told when people join or leave.",
        );
    } else {
        text.push_str(&format!(
            "On the call with you: {}. You will be told when people join or leave.",
            others.join("; ")
        ));
    }
    let context = context.trim();
    if !context.is_empty() {
        text.push_str(&format!(
            "\n\nWhat {name} wants you to know for this call:\n{context}"
        ));
    }
    text
}

struct PendingMessage {
    kind: String,
    text: String,
    delegation_id: Option<String>,
}

struct Bridge {
    reporter: Reporter,
    ringid: String,
    silicon: Account,
    /// What a replacement GPT-Live session needs.
    api_key: String,
    voice: String,
    context: String,
    resumed_at: Option<Instant>,
    roster: Roster,
    captions: Captions,
    heard: Heard,
    /// Delegations waiting out their grace period: (Ring id, GPT-Live offset, due).
    delegations: Vec<(String, Option<i64>, Instant)>,
    /// Ring delegation id → GPT-Live delegation id.
    delegation_ids: HashMap<String, String>,
    /// Messages sent to GPT-Live and not yet acknowledged, by message id.
    awaiting: HashMap<String, PendingMessage>,
    delivered: HashSet<String>,
}

impl Bridge {
    async fn run(
        &mut self,
        live: &mut Live,
        room: &livekit::Room,
        mut events: tokio::sync::mpsc::UnboundedReceiver<livekit::RoomEvent>,
        session: &mut Session<RepControl>,
        start: Option<&str>,
    ) -> Result<()> {
        let voice = Voice::publish(room, LIVE_RATE, MAX_VOICE_BACKLOG).await?;
        // The start text is held until someone can hear it: a callee's device may still be
        // joining the room when the representative is ready.
        let mut start = start
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| format!("Speak first. Begin by saying: {s}"));
        let start_by = Instant::now() + START_WAIT;
        self.reporter.send(FromMedia::RepresentativeStatus {
            ringid: self.ringid.clone(),
            silicon_uuid: self.silicon.uuid.clone(),
            status: RepresentativeStatus::Live,
            error: None,
        });
        tracing::info!(ringid = %self.ringid, silicon = %self.silicon.id, "representative live");

        let me = room.local_participant().identity().to_string();
        let mut feeds = Feeds::new(LIVE_RATE, Buffering::INTERACTIVE);
        let mut clock = Clock::new(LIVE_RATE);
        let mut tick = audio::tick_interval();
        let mut housekeeping = tokio::time::interval(Duration::from_millis(100));
        let mut heard_from_live = Instant::now();
        loop {
            // Room failures end the representative here; anything that comes back as an error
            // is a GPT-Live failure, which a fresh session can recover from.
            let outcome = tokio::select! {
                _ = session.stop.cancelled() => return Ok(()),
                _ = tick.tick() => self.listen(live, &feeds, &mut clock, &me).await,
                event = live.next_event() => {
                    heard_from_live = Instant::now();
                    match event {
                        Ok(event) => self.live_event(live, &voice, event).await,
                        Err(e) => Err(e),
                    }
                }
                Some(command) = session.control.recv() => self.command(live, command).await,
                event = events.recv() => {
                    let Some(event) = event else { bail!("the LiveKit room closed") };
                    match room::apply(&mut feeds, event) {
                        // ring-server ends the room with the call; call.closed follows.
                        Change::Ended => return Ok(()),
                        Change::Disconnected(reason) => bail!("disconnected from the LiveKit room ({reason})"),
                        Change::None | Change::Left(_) => Ok(()),
                    }
                }
                _ = housekeeping.tick() => {
                    self.captions.tick();
                    self.flush_delegations(false);
                    let listening = || {
                        Instant::now() >= start_by
                            || feeds.identities().any(|id| id != me && !id.starts_with("ring:"))
                    };
                    if heard_from_live.elapsed() > SILENCE_LIMIT {
                        Err(anyhow!("GPT-Live stopped responding"))
                    } else if let Some(text) = start.take_if(|_| listening()) {
                        live.append("instructions", &text, None, None).await
                    } else {
                        Ok(())
                    }
                }
            };
            if let Err(cause) = outcome {
                if !self.resume(live, cause, &session.stop).await? {
                    return Ok(());
                }
                // The new session starts its own timeline; send it audio from now on.
                clock = Clock::new(LIVE_RATE);
                heard_from_live = Instant::now();
            }
        }
    }

    /// Replaces a GPT-Live session that failed (a dropped connection, or the session's time
    /// limit) so the call keeps its representative. Returns false if stopped meanwhile.
    async fn resume(
        &mut self,
        live: &mut Live,
        cause: anyhow::Error,
        stop: &CancellationToken,
    ) -> Result<bool> {
        if self
            .resumed_at
            .is_some_and(|t| t.elapsed() < MIN_RESUME_INTERVAL)
        {
            return Err(cause);
        }
        tracing::warn!(ringid = %self.ringid, silicon = %self.silicon.id, "GPT-Live session lost, starting a new one: {cause:#}");
        // Close out everything tied to the old session's timeline.
        self.captions.finish();
        self.flush_delegations(true);
        let instructions = self.resume_instructions();
        self.heard = Heard::default();
        let (api_key, voice) = (self.api_key.clone(), self.voice.clone());
        let mut last_error = cause;
        for (attempt, delay) in RESUME_DELAYS.into_iter().enumerate() {
            let connect = async {
                tokio::time::sleep(delay).await;
                Live::connect(&api_key, &voice, &instructions).await
            };
            let connected = tokio::select! {
                _ = stop.cancelled() => return Ok(false),
                connected = connect => connected,
            };
            match connected {
                Ok(fresh) => {
                    let old = std::mem::replace(live, fresh);
                    tokio::spawn(old.close());
                    self.resumed_at = Some(Instant::now());
                    // Replies the old session never acknowledged go to the new one. It does
                    // not know the old delegations, so they are sent without one.
                    let unacknowledged: Vec<_> = self.awaiting.drain().collect();
                    for (message_id, message) in unacknowledged {
                        live.append(&message.kind, &message.text, None, Some(&message_id))
                            .await?;
                        self.awaiting.insert(
                            message_id,
                            PendingMessage {
                                delegation_id: None,
                                ..message
                            },
                        );
                    }
                    tracing::info!(ringid = %self.ringid, silicon = %self.silicon.id, "GPT-Live session resumed");
                    return Ok(true);
                }
                Err(e) => {
                    tracing::warn!(ringid = %self.ringid, attempt = attempt + 1, "GPT-Live reconnect failed: {e:#}");
                    last_error = e;
                }
            }
        }
        Err(last_error.context("could not reconnect to GPT-Live"))
    }

    fn resume_instructions(&self) -> String {
        let mut text = instructions(&self.silicon, &self.context, &self.roster);
        text.push_str(
            "\n\nThe call is already under way: your line dropped for a moment and you are back. \
             Do not greet anyone or start over. If you were cut off, say briefly that the line \
             dropped and carry on.",
        );
        if let Some(said) = self.captions.last_utterance() {
            text.push_str(&format!("\nThe last thing you said: \"{said}\""));
        }
        if let Some(heard) = self.heard.request(None) {
            text.push_str(&format!("\nThe last thing you heard: \"{heard}\""));
        }
        text
    }

    /// Sends GPT-Live the next slice of what everyone else is saying.
    async fn listen(
        &self,
        live: &mut Live,
        feeds: &Feeds,
        clock: &mut Clock,
        me: &str,
    ) -> Result<()> {
        let n = clock.due();
        if n == 0 {
            return Ok(());
        }
        let mut mix = vec![0i32; n];
        for (identity, samples) in feeds.pull(n) {
            // Ring's own helpers (recorder, voicemail player) are never part of the conversation.
            if identity != me && !identity.starts_with("ring:") && self.roster.audible(&identity) {
                audio::mix_into(&mut mix, &samples);
            }
        }
        live.append_audio(&audio::clip(&mix)).await
    }

    async fn live_event(&mut self, live: &mut Live, voice: &Voice, event: Event) -> Result<()> {
        match event {
            Event::OutputAudio(samples) => voice.push(&samples),
            Event::OutputTranscript {
                delta,
                start_ms,
                end_ms,
            } => {
                // A word's transcript arrives together with its audio, so the room hears it once
                // the audio queued ahead of it has played. GPT-Live's own timeline is no guide:
                // its audio arrives a few percent slower than real time.
                let queued = Duration::from_millis(voice.queued() as u64 * 1000 / LIVE_RATE as u64);
                self.captions
                    .delta(&delta, start_ms, end_ms, Instant::now() + queued);
            }
            Event::InputTranscript {
                delta,
                start_ms,
                end_ms,
            } => self.heard.push(delta, start_ms, end_ms),
            Event::Delegation { id, offset_ms } => {
                // Ring's ids are dlg_…; remember GPT-Live's own id for the replies.
                let ring_id = format!("dlg_{}", id.strip_prefix("item_").unwrap_or(&id));
                self.delegation_ids.insert(ring_id.clone(), id);
                let due = Instant::now() + DELEGATION_GRACE;
                self.delegations.push((ring_id, offset_ms, due));
            }
            Event::Appended {
                client_event_id: Some(id),
            } => {
                if self.awaiting.remove(&id).is_some() {
                    self.delivered.insert(id.clone());
                    self.reporter.send(FromMedia::MessageDelivered {
                        ringid: self.ringid.clone(),
                        message_id: id,
                    });
                }
            }
            Event::Error {
                code,
                message,
                client_event_id,
            } => {
                self.rejected(live, client_event_id, &code, &message)
                    .await?
            }
            Event::Closed { reason } => bail!("GPT-Live ended the session ({reason})"),
            Event::Appended { .. } | Event::Other => {}
        }
        Ok(())
    }

    async fn command(&mut self, live: &mut Live, command: RepControl) -> Result<()> {
        match command {
            RepControl::Message {
                message_id,
                kind,
                text,
                delegation_id,
            } => {
                if self.delivered.contains(&message_id) {
                    // A retry from ring-server of something GPT-Live already has.
                    self.reporter.send(FromMedia::MessageDelivered {
                        ringid: self.ringid.clone(),
                        message_id,
                    });
                    return Ok(());
                }
                if self.awaiting.contains_key(&message_id) {
                    // Already sent; its acknowledgement reports the delivery.
                    return Ok(());
                }
                let delegation_id = delegation_id.map(|id| self.live_delegation_id(&id));
                live.append(
                    kind.as_str(),
                    &text,
                    delegation_id.as_deref(),
                    Some(&message_id),
                )
                .await?;
                self.awaiting.insert(
                    message_id,
                    PendingMessage {
                        kind: kind.as_str().into(),
                        text,
                        delegation_id,
                    },
                );
            }
            RepControl::Roster {
                roster,
                joined,
                left,
            } => {
                self.roster.replace(roster);
                for (account, verb) in [(joined, "joined"), (left, "left")] {
                    if let Some(account) = account.filter(|a| a.uuid != self.silicon.uuid) {
                        let who = describe(account.display_name.as_deref(), &account.id);
                        live.append(
                            "instructions",
                            &format!("{who} {verb} the call."),
                            None,
                            None,
                        )
                        .await?;
                    }
                }
            }
            RepControl::Authority {
                account_uuid,
                identity,
            } => self.roster.set_authority(&account_uuid, &identity),
        }
        Ok(())
    }

    fn live_delegation_id(&self, ring_id: &str) -> String {
        match self.delegation_ids.get(ring_id) {
            Some(id) => id.clone(),
            None => match ring_id.strip_prefix("dlg_") {
                Some(rest) => format!("item_{rest}"),
                None => ring_id.to_owned(),
            },
        }
    }

    /// GPT-Live rejected an event. A message tied to a delegation it no longer knows (for
    /// example after a representative restart) is resent without the delegation so the silicon's
    /// reply still reaches the conversation.
    async fn rejected(
        &mut self,
        live: &mut Live,
        event_id: Option<String>,
        code: &str,
        message: &str,
    ) -> Result<()> {
        let Some(pending) = event_id.as_ref().and_then(|id| self.awaiting.remove(id)) else {
            tracing::warn!(ringid = %self.ringid, code, "GPT-Live reported an error: {message}");
            return Ok(());
        };
        let message_id = event_id.unwrap_or_default();
        if pending.delegation_id.is_some() {
            tracing::info!(ringid = %self.ringid, %message_id, code, "resending a message without its delegation");
            live.append(&pending.kind, &pending.text, None, Some(&message_id))
                .await?;
            self.awaiting.insert(
                message_id,
                PendingMessage {
                    delegation_id: None,
                    ..pending
                },
            );
        } else {
            tracing::warn!(ringid = %self.ringid, %message_id, code, "GPT-Live rejected a message: {message}");
        }
        Ok(())
    }

    fn flush_delegations(&mut self, all: bool) {
        let now = Instant::now();
        let (due, waiting): (Vec<_>, Vec<_>) = std::mem::take(&mut self.delegations)
            .into_iter()
            .partition(|(_, _, at)| all || *at <= now);
        self.delegations = waiting;
        for (delegation_id, offset_ms, _) in due {
            let request = self
                .heard
                .request(offset_ms)
                .or_else(|| self.captions.last_utterance());
            self.reporter.send(FromMedia::DelegationCreated {
                ringid: self.ringid.clone(),
                silicon_uuid: self.silicon.uuid.clone(),
                delegation_id,
                request,
            });
        }
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        self.flush_delegations(true);
    }
}

/// What GPT-Live heard from the others, on its own timeline, to describe what a delegation is about.
#[derive(Default)]
struct Heard {
    words: VecDeque<(i64, i64, String)>,
}

/// Pauses longer than this split what was heard into separate requests.
const HEARD_GAP_MS: i64 = 1500;

impl Heard {
    fn push(&mut self, delta: String, start_ms: i64, end_ms: i64) {
        self.words.push_back((start_ms, end_ms, delta));
        while self
            .words
            .front()
            .is_some_and(|(start, _, _)| *start < end_ms - 120_000)
        {
            self.words.pop_front();
        }
    }

    /// The run of speech that ended closest before `offset_ms`.
    fn request(&self, offset_ms: Option<i64>) -> Option<String> {
        let limit = offset_ms.map_or(i64::MAX, |o| o + 1000);
        let words: Vec<_> = self
            .words
            .iter()
            .filter(|(start, _, _)| *start <= limit)
            .collect();
        let mut first = words.len().checked_sub(1)?;
        while first > 0 && words[first].0 - words[first - 1].1 <= HEARD_GAP_MS {
            first -= 1;
        }
        let text: String = words[first..].iter().map(|(_, _, w)| w.as_str()).collect();
        let text = text.trim();
        (!text.is_empty()).then(|| text.to_owned())
    }
}

/// Segments the representative's output transcript into utterances.
struct Captions {
    reporter: Reporter,
    ringid: String,
    account_uuid: String,
    /// Offsets are measured from when the representative joined the room.
    joined_at: Instant,
    current: Option<Utterance>,
    last: Option<String>,
}

struct Utterance {
    segment_id: String,
    text: String,
    /// GPT-Live timeline, for finding pauses.
    end_ms: i64,
    /// When the room hears the first word.
    heard_at: Instant,
    updated: Instant,
    partial_at: Option<Instant>,
    dirty: bool,
}

/// A pause this long on GPT-Live's timeline after a finished sentence starts a new utterance;
/// mid-sentence it takes `CAPTION_LONG_GAP_MS`.
const CAPTION_GAP_MS: i64 = 1200;
const CAPTION_LONG_GAP_MS: i64 = 3000;
/// No new words for this long ends the utterance (`CAPTION_LONG_IDLE` mid-sentence).
const CAPTION_IDLE: Duration = Duration::from_millis(1500);
const CAPTION_LONG_IDLE: Duration = Duration::from_secs(3);
const PARTIAL_INTERVAL: Duration = Duration::from_millis(300);

impl Utterance {
    fn sentence_ended(&self) -> bool {
        live::spoken(&self.text).ends_with(['.', '?', '!', '…', '。', '？', '！'])
    }

    /// Whether a pause of `gap_ms` (timeline) or `idle` (no new words) ends this utterance.
    fn ended_by(&self, gap_ms: i64, idle: Duration) -> bool {
        if self.sentence_ended() {
            gap_ms > CAPTION_GAP_MS || idle >= CAPTION_IDLE
        } else {
            gap_ms > CAPTION_LONG_GAP_MS || idle >= CAPTION_LONG_IDLE
        }
    }
}

impl Captions {
    fn new(reporter: Reporter, ringid: String, account_uuid: String, joined_at: Instant) -> Self {
        Self {
            reporter,
            ringid,
            account_uuid,
            joined_at,
            current: None,
            last: None,
        }
    }

    fn delta(&mut self, delta: &str, start_ms: i64, end_ms: i64, heard_at: Instant) {
        if self
            .current
            .as_ref()
            .is_some_and(|u| u.ended_by(start_ms - u.end_ms, Duration::ZERO))
        {
            self.finish();
        }
        let utterance = self.current.get_or_insert_with(|| Utterance {
            segment_id: new_segment_id(),
            text: String::new(),
            end_ms,
            heard_at,
            updated: Instant::now(),
            partial_at: None,
            dirty: false,
        });
        utterance.text.push_str(delta);
        utterance.end_ms = utterance.end_ms.max(end_ms);
        utterance.updated = Instant::now();
        utterance.dirty = true;
        self.partial();
    }

    fn tick(&mut self) {
        if self
            .current
            .as_ref()
            .is_some_and(|u| u.ended_by(0, u.updated.elapsed()))
        {
            self.finish();
        } else {
            self.partial();
        }
    }

    fn partial(&mut self) {
        let Some(u) = self.current.as_mut() else {
            return;
        };
        if !u.dirty || u.partial_at.is_some_and(|t| t.elapsed() < PARTIAL_INTERVAL) {
            return;
        }
        let text = live::spoken(&u.text);
        if text.is_empty() {
            return;
        }
        u.dirty = false;
        u.partial_at = Some(Instant::now());
        self.reporter.send(FromMedia::SpeechPartial {
            ringid: self.ringid.clone(),
            account_uuid: self.account_uuid.clone(),
            segment_id: u.segment_id.clone(),
            text,
        });
    }

    fn finish(&mut self) {
        let Some(u) = self.current.take() else { return };
        let text = live::spoken(&u.text);
        if text.is_empty() {
            return;
        }
        self.last = Some(text.clone());
        self.reporter.send(FromMedia::SpeechFinal {
            ringid: self.ringid.clone(),
            account_uuid: self.account_uuid.clone(),
            segment_id: u.segment_id,
            text,
            offset_ms: Some(signed_ms(u.heard_at, self.joined_at).max(0)),
        });
    }

    fn last_utterance(&self) -> Option<String> {
        self.current
            .as_ref()
            .map(|u| live::spoken(&u.text))
            .filter(|t| !t.is_empty())
            .or_else(|| self.last.clone())
    }
}
