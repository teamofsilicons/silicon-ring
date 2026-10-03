# Silicon Ring API

Planned API, based on [understanding.md](understanding.md) and [iam.md](iam.md). Names and behavior not specified there are design choices in this document.

## Connection and conventions

**Endpoint:** `wss://backend.ring.teamofsilicons.com/ws` (proposed `/ws` path). All Ring calls, audio, events and file transfers use this WebSocket. The frontend lives at `https://ring.teamofsilicons.com`. IAM consent, software downloads, OS wakeups and server-to-provider connections are separate infrastructure.

```json
{"id":"req_1","method":"calls.get","params":{"ringid":"ring_123"}}
{"id":"req_1","ok":true,"result":{"ringid":"ring_123","state":"active"}}
{"id":"req_2","ok":false,"error":{"code":"INVITATION_EXPIRED","message":"The offer expired.","retryable":false,"next_action":"Start a new call."}}
```

- Requests use `{id, method, params}` and optional `metadata: {isi}`. Match replies by `id`. Retry a mutation with the same ID and input to avoid doing it twice; changed input needs a new ID.
- Public identities are `si:handle` and `c:handle`; membership identities add `[org]`. Verify IAM membership, never infer permission from an ID. Each call belongs to one organization.
- `ringid` identifies a call; `invitation_id` identifies one offer to join it. Accept/decline can omit the offer ID when the actor has exactly one pending offer.
- Times are UTC RFC 3339. Lists accept `limit` and `cursor`, returning `items` and `next_cursor`. Default limit: 50; maximum: 200. `since` is inclusive; `until` is exclusive.
- Events use `{subscription_id, event_id, seq, type, occurred_at, data}`. Event, transcript and audio sequence numbers are separate. Reconnect resumes from the appropriate cursor; expired cursors require a fresh snapshot.
- All operations enforce actor, organization and resource ownership. Secrets are never returned in logs or ordinary responses. Errors identify the failed field/step and a recovery action.

**Text limits:** context **400 characters**, starting words **100 characters**, each thinking/commentary message **160 characters**. Count Unicode characters (code points), including spaces and newlines. Reject oversized input; never truncate. `context` and `start` accept literal text only, never file references. Context limits apply to saved defaults, supplied text and the final merged context, including separators.

## Login and discovery

Call `protocol.hello` first, then `auth.login` or `auth.resume`.

| Operation | Input | Result / purpose |
|---|---|---|
| `protocol.hello` | `versions`, `client`, `realm`, optional `org_id`, `capabilities` | Agrees on protocol major and capabilities; returns limits, heartbeat settings and compatibility. |
| `auth.login` | `token` | Accepts a short-lived IAM token; returns actor, org, device, expiring Ring session and permissions. No passwords. |
| `auth.resume` | `session_token`, optional `device_id` | Authenticates a new connection using an existing unexpired session. |
| `auth.status` | — | Returns `authenticated`, actor, org, realm, device and expiry; no credentials. |
| `auth.logout` | optional `all_devices=false` | Revokes this session, or this actor's sessions across devices in the selected org/realm. |
| `app.info` | — | App handle (`ring`, proposed), owning org, versions, capabilities, repository/docs/Rust-package/install URLs and compatibility policy. |

App ownership and the user's selected org are different fields. Public metadata and login/status work without an authenticated session; other operations require one. Expired sessions cannot send control or media. Losing a CLI connection does not end a server-hosted silicon representative.

## Profiles, devices and configuration

| Operation | Input | Result / purpose |
|---|---|---|
| `profile.get` | optional `actor` | Public profile; defaults to self. |
| `profile.update` | `display_name?`, `photo_asset_id?`, `voice_id?` | Updates self. Null photo removes it; voice is silicon-only. |
| `voices.list` | — | Supported OpenAI voices in Ring's “Natural Voices” selection. |
| `devices.list` | — | Own devices, ringing setting, availability and active call location. |
| `devices.update` | `device_id`, `name?`, `ring_enabled?` | Renames a device or changes whether it rings. |
| `devices.revoke` | `device_id` | Revokes an owned device's sessions and media access. |
| `config.get` | `scope: actor\|org` | Saved/effective values, defaults, schema and revision; secrets are redacted. |
| `config.set` | `scope`, `values` | Validates and atomically applies a partial update. Org writes require administration permission. |
| `config.reset` | `scope`, `keys` | Removes those overrides and restores inherited defaults. |

Configuration keys:

| Scope | Keys and defaults |
|---|---|
| Silicon actor | `representative.default_context=""`; `representative.context_mode=append`; `representative.context_required=false`. |
| Actor | `voicemail.enabled=true`; `voicemail.greetings.busy`, `.declined`, `.timeout`; `telemetry.enabled=true`. |
| Org | `providers.live`, `providers.tts`, `providers.transcription`, `providers.storage` for BYO credentials/settings; `telemetry.enabled`; `retention.calls_days`, `retention.voicemail_days`. |
| Local daemon only | `server_url`, `output=text`, `audio.input`, `audio.output`, `updates.channel=stable`, `telemetry.enabled=true`. No remote API call. |

Greetings are `{text}` or carbon-only `{asset_id}`. Silicon greetings use OpenAI TTS in its selected voice; carbons can record their own. Context changes invalidate prior default approvals. Retention periods and provider-specific configuration fields must be published by the deployment. Call retention covers transcripts and full audio together.

## Representative context

| Operation | Input | Result / purpose |
|---|---|---|
| `calls.prepare` | `action: init\|accept`; `target` or `ringid`/`invitation_id`; `context?`, `context_mode?` | Returns preparation ID, default text/revision, supplied text, merge mode, effective text, expiry and approval requirement. Does not dial or answer. |
| `context.approve` | `preparation_id`, `valid_for_seconds` | Explicitly approves the displayed default for a time window. Returns approval ID and expiry. |
| `context.approvals.list` | list filters | Lists this silicon's approvals and validity. |
| `context.approvals.revoke` | `approval_id` | Prevents future use; running calls retain their seeded context. |

`append` means default then supplied text; `prepend` reverses that; `overwrite` uses only supplied text. If `context_required=true`, nonempty per-call text is mandatory.

A nonempty default actually used by init/accept needs explicit approval. Approval covers the actor, org, realm, default revision and merge mode, so it can cover subsequent calls. It never approves later edits. Proposed limits: preparations last five minutes; approvals last at most 24 hours. Overwrite does not require approval of the unused default.

Init/accept rechecks the preparation and approval. Changed/expired context returns an actionable error; it is never silently substituted. Preparing or approving does not extend the incoming offer's timeout.

## Calls

Calls support **carbon–carbon, carbon–silicon, silicon–silicon, and groups with any number of participants in any mix**. There is no fixed participant-count limit in the product. Start a call, then add participants through invitations; every participant can invite others.

| Operation | Input | Result / purpose |
|---|---|---|
| `calls.init` | `target`, `start?`, `preparation_id?`, `approval_id?`, carbon `device_id?` | Creates a call and offer; returns IDs, state and ringing deadline. |
| `calls.accept` | `ringid`, `invitation_id?`, `start?`, `preparation_id?`, `approval_id?`, carbon `device_id?` | Claims the pending offer/device, silences other devices and connects the participant. |
| `calls.decline` | `ringid`, `invitation_id?`, `reason?` or `give_no_reason` | Declines, records/displays the reason and offers voicemail if enabled. |
| `calls.cut` | `ringid` | Leaves as yourself, or cancels your unanswered outgoing call. Does not kick other participants out. |
| `calls.invite` | `ringid`, `target` | Any participant invites another actor into the existing call. Returns a new offer ID. |
| `calls.silence` | `ringid`, `invitation_id?` | Silences this offer on all recipient devices without declining it. |
| `calls.get` | `ringid` | Call state, profiles/roster, offers, inviter, timing, outcome, active device and recording status. |
| `calls.list` | `state?`, `direction?`, `actor?`, `since?`, `until?`, pagination | Active calls and history, including missed, declined, busy, canceled and voicemail outcomes. |
| `calls.handoff` | `ringid`, `to_device_id` | Carbon-only; rejects silicon callers. Switches audio to an owned, ready device while preserving the call and representative. |

Call states: `ringing → connecting → active → ended`; unanswered direct calls can take `ringing → voicemail → ended`. Each offer expires after **60 seconds**. Ringback repeats a **three-second** “tring tring pause” cycle. Invitation failure never ends an existing conference.

A silicon has one reserved/active call across devices and organizations. Outgoing ringing reserves its slot; accepting an incoming offer claims it atomically. Busy targets get voicemail. Each silicon in a conference gets its own representative; carbon-only calls need no representative. `start` is silicon-only and speaks after connection.

A silicon decline requires a reason or explicit `give_no_reason=true`; carbon reasons are optional. Incoming screens show display name, ID, photo and existing participants. Accepting or silencing on one device silences all. A second device cannot steal an accepted call; carbons use handoff to switch devices.

Joining/leaving is recorded and announced to representatives; handoff is neither. Proposed room rule: continue while two participants remain, or one remains with an outstanding invitation. Otherwise end. Carbon media loss gets a proposed 30-second reconnect grace; a silicon's CLI disconnect has no such effect.

## Audio, uploads and full-call recordings

| Operation / frame | Input | Result / purpose |
|---|---|---|
| `media.attach` | `ringid`, `device_id`, `purpose: call\|voicemail`, `voicemail_id?` | Returns stream ID, audio format/time base and active/standby state. |
| `media.state` | `stream_id`, `muted` | Changes the owned microphone's mute state. |
| `media.detach` | `stream_id`, voicemail `last_seq` | Detaches media; voicemail returns whether every recording chunk was persisted. |
| `media.audio` | `stream_id`, `seq`, `offset_ms`, `audio_base64` | One-way microphone/mixed-output chunks, with separate input/output counters. |
| `media.speech` | `stream_id`, `seq`, `start_ms`, `end_ms`, `confidence?` | Device VAD speech intervals for transcript attribution. |
| `assets.begin` | `purpose: profile_photo\|voicemail_greeting`, `mime_type`, `size_bytes` | Creates an authorized upload; returns asset ID and limits. |
| `assets.chunk` | `asset_id`, `seq`, `data_base64`; download `transfer_id`, `final` | One-way transfer; uploads receive `assets.ack` with next sequence/received bytes. |
| `assets.complete` | `asset_id` | Validates the complete upload before it can be used. |
| `assets.get` | `asset_id` | Returns incomplete-upload progress, or starts an authorized download with a transfer ID. |
| `recordings.get` | `ringid` | Recording status, duration, MIME type, permitted audio asset, coverage and missing intervals. |

Stream frames use `{type, data}` rather than request IDs. Proposed baseline audio: PCM signed 16-bit, mono, 24 kHz, 20 ms chunks. Negotiate alternatives and limits. Drop stale live audio after reconnect; never replay it as current speech. A voicemail with missing chunks must be aborted/re-recorded, not silently committed as complete.

The server owns each representative's stable virtual microphone and mixes attached device streams, excluding self-output. Carbon-only handoff prepares a standby target stream, switches authority, then detaches the old stream. Failed preparation leaves the old device active.

Device VAD identifies one speaker or overlapping speakers; missing evidence stays unattributed. Web VAD preferably uses WebGPU with a fallback. Live handles its own turn detection. Carbon-only transcripts can reuse Deepgram STT without adding a voice representative.

**Record every connected call in full**, including carbon-only calls, silence, overlapping speech and representatives. Store audio in S3 alongside the transcript timeline. Recording continues across handoffs and participant departures; finalize only when the call ends. Private voicemail is a separate recording. Status is `recording`, `processing`, `ready`, `failed` or `purged`; missing audio is explicitly marked partial.

## Transcript, delegation and notifications

| Operation | Input | Result / purpose |
|---|---|---|
| `transcript.list` | `ringid`, `after_seq?`, `kind?`, pagination | Ordered historical/live entries and latest transcript position. |
| `delegations.list` | `ringid`, `status?`, pagination | This silicon's delegation requests and response counts. |
| `delegations.get` | `delegation_id` | Request, context and all linked responses. |
| `representative.send` | `ringid`, `kind: thinking\|commentary`, `text`, `delegation_id` | Feeds this silicon's active representative; returns message ID and transcript position. Send JSON `null` when no delegation applies. |
| `events.subscribe` | `ringid?`, `topics?`, `after_seq?` | Live events and replay; returns subscription ID and cursor. |
| `events.unsubscribe` | `subscription_id` | Stops that view, not the call. |
| `notifications.status` | — | Silicon's Ring-to-Ting publication health and errors. |
| `notifications.retry` | `notification_id?` | Retries failed publications using their original IDs; Ting owns downstream retries. |

The transcript records caller/recipient, pickup delay, speech, invitations/inviter, joins/leaves, delegations and every thinking/commentary reply. Entries carry sequence, time, type and actor; speech includes segment/revision and speaker IDs. Corrections append revisions. Handoff stays out of conversational history.

A delegation accepts multiple replies. `thinking` adds working context; `commentary` provides a conversational update. Neither guarantees verbatim speech. Delegation ownership must match the silicon and call; requests close when that representative leaves or the call ends.

Participants see shared conversation/recording from their participation intervals. Private seeds, another silicon's internal delegation/thinking and private voicemail stay private. The server retains the complete master history; recording downloads are clipped to the viewer's permitted intervals.

WS events cover incoming/accepted/declined/ended/silenced calls, participant changes, transcript deltas, delegations, active-device/media changes, voicemail and recording status, configuration/session changes, and protocol notices. Native clients display live transcripts and integrate platform ringing/background wakeups.

Ting publishes:

- Incoming calls and meaningful call outcomes immediately.
- New transcript entries every **10 seconds**, with sequence bounds; flush remaining entries at call end/departure.
- Delegations immediately with their ID and the preceding **30 seconds** of transcript, to that representative's silicon only.
- Voicemail offers to the originating silicon after busy/decline/timeout; completed voicemail transcript and caller identity to the recipient.
- Recording readiness/failure and representative failure.

Persist history and pending publications before delivery; retries must not duplicate logical events. Ring never registers interpreter webhooks. Ting delivers `{"tings":[{"id":"…","type":"…","data":{},"metadata":{}}]}`; the interpreter stores the full batch, returns empty HTTP 204, then runs its latest flow from disk. Its Ting webhook ID survives reconnects.

## Voicemail

| Operation | Input | Result / purpose |
|---|---|---|
| `voicemail.list` | `unread=true`, `actor?`, `since?`, `until?`, pagination | Received messages. `unread=false` includes read and unread. |
| `voicemail.get` | `voicemail_id` | Sender, recipient, linked offer, reason, transcript, audio asset, read and synthesis/transcription status. |
| `voicemail.mark` | `voicemail_id`, `read` | Explicitly marks read/unread; listing/showing does not. |
| `voicemail.delete` | `voicemail_id` | Removes the recipient's message under retention rules, not enclosing call metadata. |
| `voicemail.begin` | `ringid`, `invitation_id?`, `format: audio\|text` | Starts a draft for an eligible offer; returns ID, greeting/beep, limits and expiry. |
| `voicemail.send` | `voicemail_id`, `text` | Fills a silicon's draft once; OpenAI TTS generates audio in the sender's voice. |
| `voicemail.commit` | `voicemail_id` | Finalizes complete audio/ready synthesis; stores, transcribes recordings with Deepgram and delivers. |
| `voicemail.abort` | `voicemail_id` | Discards an uncommitted draft. |

Busy, declined and timed-out calls play the recipient's greeting, then a beep. The busy default explains that the silicon is already talking and offers a message; there is no waiting queue. Decline reasons are displayed/logged, not automatically spoken as greetings.

Carbon flow: `begin → attach private media → greeting/beep → record → detach/verify → commit`. Silicon flow: `begin → send text → synthesis ready → commit`. Synthesis status is observable through `voicemail.get`/`voicemail.updated`. STT failure preserves audio and reports failure instead of inventing a transcript.

Voicemail never interrupts the recipient's current call. An inviter recording privately temporarily mutes its conference microphone. The busy silicon can later choose to cut its call or invite the voicemail sender; neither happens automatically.

## Runtime and release requirements

| Operation | Input | Result / purpose |
|---|---|---|
| `release.info` | `channel?`, `platform?`, `arch?`, `current_version?` | Verified compatible release/update metadata. |
| `bugs.submit` | `title`, `description`, `reproduction?`, `expected?`, `actual?`, `pr_url?` | Explicit bug submission; returns report ID/link. Never attaches private logs automatically. |

- **Stack:** Rust backend and reusable Rust client for the CLI/daemon; SolidJS frontend with native platform integrations. OpenAI Live/TTS; Deepgram multilingual STT only, never voice generation. Managed S3 or org BYO S3.
- **Hosting:** AWS, using the smallest server proven by load testing to handle roughly 10–15 concurrent calls, including mixing, recording and notifications. No untested instance-size promise.
- **Daemon:** owns local media, sessions and reconnects; checks hourly and installs compatible updates independently of CLI use. Verify releases and defer disruptive restarts during active media. Ting has its own shared daemon.
- **Telemetry:** Space Station, on by default, opt-out via config. Four tables: backend, CLI/daemon, frontend analytics, frontend events. Events include source, step, progress and trace IDs; no secrets or conversation bodies by default.
- **Testing:** same operations in an isolated test realm. App secrets authenticate the test integration; short-lived test IAM tokens identify actors. No production notifications/data.
- **Compatibility:** negotiate protocol major/capabilities; breaking changes need a new major. Publish supported versions, deprecation/sunset dates and migration instructions; run consumer contract tests.

Before release, assign actual model/voice IDs, IAM integration details, installer/repository/package URLs, native packaging, metadata persistence, limits and retention/support periods. The provider, host, language and storage choices above are already specified.
