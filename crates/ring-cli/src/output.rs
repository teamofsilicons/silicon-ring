//! Printing: compact JSON with `--json`, short readable text otherwise.

use std::io::Write;

use ring_proto::{
    Account, Call, CallState, ContextApproval, ContextPreview, Delegation, Device, EntryKind,
    Invitation, InvitationState, Participant, ParticipantState, Settings, Time, TranscriptEntry,
    Voicemail,
};
use serde::Serialize;

/// Writes one line to stdout. A closed pipe (`ring docs | head`) is not an error.
pub fn line(text: &str) {
    let mut stdout = std::io::stdout().lock();
    let _ = writeln!(stdout, "{text}");
    let _ = stdout.flush();
}

/// Writes one line to stderr.
pub fn note(text: &str) {
    let mut stderr = std::io::stderr().lock();
    let _ = writeln!(stderr, "{text}");
}

pub fn json<T: Serialize + ?Sized>(value: &T) {
    line(&serde_json::to_string(value).expect("output always serializes"));
}

/// One streamed `--json` line, `{"type","data"}` in the same shape as the WebSocket events.
pub fn event_line<T: Serialize + ?Sized>(kind: &str, data: &T) {
    #[derive(Serialize)]
    struct Line<'a, T: ?Sized> {
        r#type: &'a str,
        data: &'a T,
    }
    json(&Line { r#type: kind, data });
}

/// Prints `value` as JSON, or the text `render` builds.
pub fn emit<T: Serialize + ?Sized>(as_json: bool, value: &T, render: impl FnOnce() -> String) {
    if as_json {
        json(value);
    } else {
        let text = render();
        if !text.is_empty() {
            line(text.trim_end());
        }
    }
}

// ---- small formatters ---------------------------------------------------------------------

pub fn who(account: &Account) -> String {
    match account
        .display_name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
    {
        Some(name) if name != account.id => format!("{name} ({})", account.id),
        _ => account.id.clone(),
    }
}

pub fn time(t: &Time) -> String {
    t.0.format("%Y-%m-%d %H:%M:%SZ").to_string()
}

pub fn duration_ms(ms: u64) -> String {
    let seconds = ms / 1000;
    match seconds {
        0..=59 => format!("{}.{}s", seconds, (ms % 1000) / 100),
        60..=3599 => format!("{}m {:02}s", seconds / 60, seconds % 60),
        _ => format!("{}h {:02}m", seconds / 3600, (seconds % 3600) / 60),
    }
}

fn offset(ms: i64) -> String {
    let seconds = ms.max(0) / 1000;
    if seconds >= 3600 {
        format!(
            "{}:{:02}:{:02}",
            seconds / 3600,
            (seconds % 3600) / 60,
            seconds % 60
        )
    } else {
        format!("{:02}:{:02}", seconds / 60, seconds % 60)
    }
}

/// Indents every line of `text` by two spaces.
pub fn indent(text: &str) -> String {
    text.lines()
        .map(|l| format!("  {l}"))
        .collect::<Vec<_>>()
        .join("\n")
}

// ---- calls --------------------------------------------------------------------------------

pub fn call_state(call: &Call) -> String {
    match (call.state, call.end_reason) {
        (CallState::Ended, Some(reason)) => format!("ended ({})", reason.as_str()),
        (state, _) => state.as_str().to_owned(),
    }
}

/// The people on the other side of `call` from `me`.
pub fn others(call: &Call, me: &str) -> String {
    let mut names: Vec<String> = Vec::new();
    let mut push = |account: &Account| {
        if account.uuid != me && !names.iter().any(|n| n == &who(account)) {
            names.push(who(account));
        }
    };
    push(&call.created_by);
    for p in &call.participants {
        push(&p.account);
    }
    for i in &call.invitations {
        push(&i.to);
    }
    if names.is_empty() {
        "nobody else".into()
    } else {
        names.join(", ")
    }
}

pub fn call_row(call: &Call, me: &str) -> String {
    let arrow = match call.direction {
        ring_proto::Direction::Incoming => "←",
        ring_proto::Direction::Outgoing => "→",
    };
    let length = match (call.answered_at, call.ended_at) {
        (Some(a), Some(e)) => format!("  {}", duration_ms((e.millis() - a.millis()).max(0) as u64)),
        _ => String::new(),
    };
    format!(
        "{}  {}  {arrow} {}  {}{length}",
        call.ringid,
        time(&call.created_at),
        others(call, me),
        call_state(call)
    )
}

pub fn call_details(call: &Call) -> String {
    let mut out = vec![format!("{}  {}", call.ringid, call_state(call))];
    out.push(format!(
        "started    {} by {}",
        time(&call.created_at),
        who(&call.created_by)
    ));
    if let Some(answered) = call.answered_at {
        let after = duration_ms((answered.millis() - call.created_at.millis()).max(0) as u64);
        out.push(format!("answered   {} (after {after})", time(&answered)));
    }
    if let Some(ended) = call.ended_at {
        let length = call.answered_at.map(|a| {
            format!(
                ", {} long",
                duration_ms((ended.millis() - a.millis()).max(0) as u64)
            )
        });
        out.push(format!(
            "ended      {}{}",
            time(&ended),
            length.unwrap_or_default()
        ));
    }
    if !call.participants.is_empty() {
        out.push("participants".into());
        out.extend(call.participants.iter().map(participant));
    }
    if !call.invitations.is_empty() {
        out.push("invitations".into());
        out.extend(call.invitations.iter().map(invitation));
    }
    let mut recording = format!("recording  {}", recording_status(call));
    if let Some(ms) = call.recording.duration_ms {
        recording.push_str(&format!(" ({})", duration_ms(ms)));
    }
    out.push(recording);
    if let Some(vm) = &call.voicemail_id {
        out.push(format!("voicemail  {vm}"));
    }
    out.join("\n")
}

fn recording_status(call: &Call) -> &'static str {
    use ring_proto::RecordingStatus as R;
    match call.recording.status {
        R::None => "none",
        R::Recording => "recording",
        R::Processing => "processing",
        R::Ready => "ready",
        R::Failed => "failed",
    }
}

fn participant(p: &Participant) -> String {
    let state = match p.state {
        ParticipantState::Joined => "joined",
        ParticipantState::Left => "left",
    };
    let mut row = format!("  {}  {state} {}", who(&p.account), time(&p.joined_at));
    if let Some(left) = p.left_at {
        row.push_str(&format!(", left {}", time(&left)));
    }
    if let Some(rep) = &p.representative {
        let status = match rep.status {
            ring_proto::RepresentativeStatus::Starting => "starting",
            ring_proto::RepresentativeStatus::Live => "live",
            ring_proto::RepresentativeStatus::Failed => "failed",
            ring_proto::RepresentativeStatus::Ended => "ended",
        };
        row.push_str(&format!("  representative {status}, voice {}", rep.voice));
    }
    if let Some(device) = &p.device {
        row.push_str(&format!(
            "  on {}",
            device.name.as_deref().unwrap_or(&device.device_id)
        ));
    }
    if let Some(by) = &p.invited_by {
        row.push_str(&format!("  invited by {}", by.id));
    }
    row
}

pub fn invitation(i: &Invitation) -> String {
    let mut row = format!(
        "  {}  {} → {}  {}",
        i.invitation_id,
        i.from.id,
        who(&i.to),
        i.state.as_str()
    );
    if i.state == InvitationState::Ringing {
        row.push_str(&format!(" until {}", time(&i.expires_at)));
    }
    if let Some(reason) = &i.reason {
        row.push_str(&format!(": {reason}"));
    }
    row
}

// ---- transcript ---------------------------------------------------------------------------

pub fn entry(e: &TranscriptEntry) -> String {
    let at = e
        .offset_ms
        .map(offset)
        .unwrap_or_else(|| e.at.0.format("%H:%M:%S").to_string());
    let actor = e.actor.as_ref().map(who).unwrap_or_else(|| "Ring".into());
    let text = e.text.as_deref().unwrap_or("");
    let data = |key: &str| e.data.get(key).and_then(|v| v.as_str()).map(str::to_owned);
    let body = match e.kind {
        EntryKind::Speech => format!("{actor}: {text}"),
        EntryKind::CallStarted => format!("· call started by {actor}"),
        EntryKind::Answered => {
            let after = e
                .data
                .get("after_ms")
                .and_then(|v| v.as_u64())
                .map(|ms| format!(" after {}", duration_ms(ms)));
            format!("· answered by {actor}{}", after.unwrap_or_default())
        }
        EntryKind::Joined => format!("· {actor} joined"),
        EntryKind::Left => format!("· {actor} left"),
        EntryKind::Invited => {
            let invitee = e
                .data
                .get("invitee")
                .and_then(|v| serde_json::from_value::<Account>(v.clone()).ok());
            format!(
                "· {actor} invited {}",
                invitee
                    .as_ref()
                    .map(who)
                    .unwrap_or_else(|| "someone".into())
            )
        }
        EntryKind::Declined => match data("reason").or_else(|| e.text.clone()) {
            Some(reason) => format!("· {actor} declined: {reason}"),
            None => format!("· {actor} declined"),
        },
        EntryKind::Missed => format!("· missed by {actor}"),
        EntryKind::Busy => format!("· {actor} was busy"),
        EntryKind::Delegation => format!(
            "» delegation {}: {}",
            data("delegation_id").unwrap_or_default(),
            data("request")
                .or_else(|| e.text.clone())
                .unwrap_or_default()
        ),
        EntryKind::Thinking => format!("  thinking{}: {text}", reply_to(e)),
        EntryKind::Commentary => format!("  commentary{}: {text}", reply_to(e)),
        EntryKind::Voicemail => format!(
            "· voicemail {} from {actor}",
            data("voicemail_id").unwrap_or_default()
        ),
        EntryKind::CallEnded => format!(
            "· call ended ({})",
            data("reason").unwrap_or_else(|| "completed".into())
        ),
        EntryKind::Note => format!("· {text}"),
    };
    format!("[{at}] {body}")
}

fn reply_to(e: &TranscriptEntry) -> String {
    e.data
        .get("delegation_id")
        .and_then(|v| v.as_str())
        .map(|id| format!(" → {id}"))
        .unwrap_or_default()
}

pub fn delegation(d: &Delegation, verbose: bool) -> String {
    let status = match d.status {
        ring_proto::DelegationStatus::Open => "open",
        ring_proto::DelegationStatus::Closed => "closed",
    };
    let mut out = vec![format!(
        "{}  {status}  {}  {}",
        d.delegation_id,
        time(&d.created_at),
        d.request.as_deref().unwrap_or("(no request text)")
    )];
    if verbose {
        out.push(format!("call {}", d.ringid));
        if !d.context.is_empty() {
            out.push("context (30 s before)".into());
            out.extend(d.context.iter().map(|e| format!("  {}", entry(e))));
        }
        if !d.replies.is_empty() {
            out.push("replies".into());
            out.extend(
                d.replies
                    .iter()
                    .map(|r| format!("  {} {}: {}", time(&r.at), r.kind.as_str(), r.text)),
            );
        }
    }
    out.join("\n")
}

// ---- context ------------------------------------------------------------------------------

/// The preview as text. `with_next` adds the approve command when one is needed.
pub fn context_preview(p: &ContextPreview, with_next: bool) -> String {
    let show = |text: &str| {
        if text.trim().is_empty() {
            "  (empty)".to_owned()
        } else {
            indent(text)
        }
    };
    let mut out = vec![
        format!("Default context (revision {}):", p.default_revision),
        show(&p.default_context),
        "Supplied context:".into(),
        show(p.supplied_context.as_deref().unwrap_or("")),
        format!("Mode: {}", p.mode.as_str()),
        format!(
            "Effective context ({} characters):",
            ring_proto::text_len(&p.effective_context)
        ),
        show(&p.effective_context),
    ];
    if let Some(approval) = &p.approval {
        out.push(format!(
            "Approved until {} ({})",
            time(&approval.expires_at),
            approval.approval_id
        ));
    } else if p.approval_required && with_next {
        out.push(format!(
            "Needs approval: ring context approve {} --for 1h",
            p.preparation_id
        ));
    } else if !p.uses_default {
        out.push("The default is not used, so no approval is needed.".into());
    }
    out.join("\n")
}

pub fn approval(a: &ContextApproval) -> String {
    format!(
        "{}  {}  revision {}  until {}",
        a.approval_id,
        a.mode.as_str(),
        a.default_revision,
        time(&a.expires_at)
    )
}

// ---- voicemail, settings, devices ---------------------------------------------------------

pub fn voicemail_row(v: &Voicemail) -> String {
    let unread = if v.read { " " } else { "•" };
    let length = v
        .duration_ms
        .map(|ms| format!(" {}", duration_ms(ms)))
        .unwrap_or_default();
    let transcript = v
        .transcript
        .as_deref()
        .map(|t| format!("  \"{}\"", preview(t, 60)))
        .unwrap_or_default();
    format!(
        "{unread} {}  {}  {}  {}{length}{transcript}",
        v.voicemail_id,
        time(&v.created_at),
        who(&v.from),
        v.reason.as_str()
    )
}

pub fn voicemail_details(v: &Voicemail) -> String {
    let mut out = vec![
        format!(
            "{}  from {}  {}",
            v.voicemail_id,
            who(&v.from),
            if v.read { "read" } else { "unread" }
        ),
        format!(
            "received   {}  ({}, call {})",
            time(&v.created_at),
            v.reason.as_str(),
            v.ringid
        ),
    ];
    if let Some(reason) = &v.decline_reason {
        out.push(format!("declined   {reason}"));
    }
    if let Some(ms) = v.duration_ms {
        out.push(format!("length     {}", duration_ms(ms)));
    }
    let audio = match v.audio_status {
        ring_proto::AudioStatus::Recording => "recording",
        ring_proto::AudioStatus::Processing => "processing",
        ring_proto::AudioStatus::Ready => "ready",
        ring_proto::AudioStatus::Failed => "failed",
    };
    out.push(format!("audio      {audio}"));
    match (&v.transcript, v.transcript_status) {
        (Some(text), _) => {
            out.push("transcript".into());
            out.push(indent(text));
        }
        (None, ring_proto::TranscriptStatus::Pending) => out.push("transcript pending".into()),
        (None, _) => out.push("transcript unavailable".into()),
    }
    out.join("\n")
}

fn preview(text: &str, max: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        flat
    } else {
        format!("{}…", flat.chars().take(max).collect::<String>())
    }
}

pub fn settings(s: &Settings) -> String {
    let greeting = |g: &Option<String>| {
        g.clone()
            .map(|t| format!("\"{t}\""))
            .unwrap_or_else(|| "(default)".into())
    };
    let rep = &s.representative;
    [
        format!("voice                            {}", s.voice),
        format!(
            "representative.default_context   {}",
            if rep.default_context.is_empty() {
                "(none)".into()
            } else {
                format!("\"{}\"", rep.default_context)
            }
        ),
        format!(
            "representative.context_mode      {}",
            rep.context_mode.as_str()
        ),
        format!("representative.context_required  {}", rep.context_required),
        format!("voicemail.enabled                {}", s.voicemail.enabled),
        format!(
            "voicemail.greetings.busy         {}",
            greeting(&s.voicemail.greetings.busy)
        ),
        format!(
            "voicemail.greetings.declined     {}",
            greeting(&s.voicemail.greetings.declined)
        ),
        format!(
            "voicemail.greetings.missed       {}",
            greeting(&s.voicemail.greetings.missed)
        ),
        format!("revision                         {}", s.revision),
    ]
    .join("\n")
}

pub fn device(d: &Device) -> String {
    let marker = if d.current { "*" } else { " " };
    let platform = d.platform.map(|p| p.as_str()).unwrap_or("unknown");
    let seen = d
        .last_seen_at
        .as_ref()
        .map(time)
        .unwrap_or_else(|| "never".into());
    let online = if d.online { "online" } else { "offline" };
    format!(
        "{marker} {}  {}  {platform}  {online}  last seen {seen}{}",
        d.device_id,
        d.name.as_deref().unwrap_or("(unnamed)"),
        if d.ring_enabled { "" } else { "  ringing off" }
    )
}
