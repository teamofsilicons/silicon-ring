# Silicon Ring CLI

Planned commands based on [understanding.md](understanding.md) and [iam.md](iam.md). The API details are in [api.md](api.md). These docs do not claim the service is already running.

## Start here

```sh
# SILICON_HOME and SILICON_ORG come from the interpreter/environment.
ring login "SHORT_LIVED_IAM_TOKEN"
ring login status --json
ring call init @c:alex --context "Discuss the launch." --start "Hi Alex!"
ring call accept ring_123 --start "Hello, how can I help?"
ring send ring_123 commentary "The answer is 42." --delegationid delegation_123
ring call cut ring_123
ring voicemail ls
ring call history
```

Normal commands return after the server accepts the action. A successful init means ringing, not answered. The server maintains silicon representatives; the daemon maintains carbon audio. `--live` watches output without owning the call.

If init/accept returns `CONTEXT_APPROVAL_REQUIRED`, inspect the displayed default and effective context, run `ring context approve PREPARATION_ID --for 1h`, then repeat the original command. Preparing or approving never places or accepts a call.

## Shared conventions

- Actor IDs: `si:handle` or `c:handle`; leading `@` is optional. Membership IDs such as `si:handle[org]` must match the selected org. `ringid` identifies a conference; `invitation_id` identifies one offer to join it.
- Global flags: `--help`, `--json`, `--org ORG`, `--test`, `--request-id ID`. Organization selection is `--org`, otherwise `SILICON_ORG`. Calls stay within that verified IAM organization.
- `SILICON_HOME/.ring` holds local configuration, session references, state, and logs. `SILICON_HOME` is required for runtime/storage operations; help/version work without it. Optional `ISI` supplies origin metadata, never identity.
- `--context TEXT` and `--start TEXT` accept inline text only, with no file or stdin option. Limits: context **400 characters**, start **100 characters**, each thinking/commentary send **160 characters**. Count Unicode characters (code points), including spaces/newlines; reject excess without truncating. Send limits also apply when using `--text-file`.
- Other text/file alternatives are mutually exclusive. `--text-file` and `--description-file` accept UTF-8 paths or `-` for stdin. Only one input may read stdin.
- List commands support `--limit N` (default 50, maximum 200) and `--cursor CURSOR`. Time filters use RFC 3339 timestamps: `--since` inclusive, `--until` exclusive. Durations accept `30s`, `15m`, `1h`.
- `--json` prints nonsecret result objects; watches/followed logs use JSON Lines. Errors include a stable code, explanation, failed step, request/resource IDs, retryability, and next action. Tokens never appear in output.
- Exit codes: `0` success/accepted, `1` operation failure, `2` invalid input, `3` authentication/permission failure, `4` conflict or required follow-up, `5` connection/daemon uncertainty, `130` interrupted foreground work. Logged-out `login status` returns `authenticated:false` successfully.
- Reuse `--request-id` only for an exact retry. Composed commands derive stable step IDs. A lost reply is not permission to create another call/message; inspect state or retry the same ID.

Every command/group has bundled offline help with purpose, flags, examples, and useful next commands. A bare group prints help without taking action.

## Accounts, profiles, devices

| Command | Action / API |
|---|---|
| `ring iam --json` | Application ID (`ring`, proposed), owning org, endpoint, repository/docs/package links, capabilities and compatibility: `app.info`. Unassigned links remain unset. |
| `ring login TOKEN` or `ring login --token-stdin` | Use an IAM-issued short-lived app token; `auth.login` stores a protected Ring session/device. Never asks for a password/provider key. |
| `ring login status --json` | Verified actor, org, device, expiry and connection status: `auth.status`. Offline verification is reported explicitly. |
| `ring logout [--all-devices]` | Revoke this session or all own sessions in the selected org/realm: `auth.logout`; clear matching local credentials. |
| `ring version` | Local CLI/daemon/build/protocol versions. |
| `ring profile show [ACTOR]` | Own or authorized public profile: `profile.get`. |
| `ring profile set [--display-name TEXT] [--photo PATH / --clear-photo] [--voice ID]` | Change own profile: `profile.update`; upload photos first. Voice is silicon-only. Historical identity snapshots remain unchanged. |
| `ring voice ls` | Supported Natural Voices and stable IDs: `voices.list`. Voice changes apply to new calls/generated greetings. |
| `ring device ls` | Own devices, ringing status and active media location: `devices.list`. |
| `ring device set ID [--name TEXT] [--ring-enabled true/false]` | Update an owned device: `devices.update`. Disabling ringing does not decline calls. |
| `ring device revoke ID` | Revoke that device's sessions/media: `devices.revoke`. |
| `ring audio devices` | Local input/output IDs, selected devices and audio permissions. These are OS devices, not Ring device IDs. |

## Configuration and context

| Command | Action / API |
|---|---|
| `ring config show [--scope local/actor/org]` | Values, defaults, origins and schema; secrets are redacted. Local read or `config.get`. Org settings require admin rights. |
| `ring config set JSON [--scope SCOPE]` or `ring config set --file PATH` | Atomic partial update; default scope `actor`. Validate keys/types before local write or `config.set`. |
| `ring config reset KEY... [--scope SCOPE]` or `ring config reset --all` | Remove overrides via local write or `config.reset`. `--all` expands the saved keys in that scope. |
| `ring context show --init ACTOR` or `ring context show --accept RINGID` | Preview through `calls.prepare`; supports `--context TEXT`, `--context-mode MODE`, and acceptance `--invitation ID`. |
| `ring context approve PREPARATION_ID [--for DURATION]` | Explicitly approve the displayed default: `context.approve`. Default 1 hour; maximum 24 hours. |
| `ring context ls` / `ring context revoke APPROVAL_ID` | Inspect/revoke own approvals: `context.approvals.list/revoke`. |

Important configuration keys:

| Scope | Keys / defaults |
|---|---|
| Actor, silicon | `representative.default_context`: empty; `representative.context_mode`: `append`; `representative.context_required`: `false`. |
| Actor | `voicemail.enabled`: `true`; `voicemail.greetings.busy/declined/timeout`: `{text}` or carbon-only `{asset_id}`; `telemetry.enabled`: `true`. |
| Org | `providers.live/tts/transcription/storage`: supported BYO credentials/settings; `retention.calls_days/voicemail_days`: deployment policy; `telemetry.enabled`: `true`. |
| Local | `server_url`, `output` (`text`), `audio.input/output` (OS defaults), `updates.channel` (`stable`), `telemetry.enabled` (`true`). |

`append` means default then call context; `prepend` reverses them; `overwrite` uses only call context. Defaults and per-call context are text strings, never file references. Each and the final merged context must fit **400 characters**, including separators. The preview shows default, explicit text, mode, effective text, revision and expiry. A used nonempty default needs approval for both init and accept. Overwrite does not use the default. `context_required=true` requires nonempty per-call context; a stored default is insufficient.

An approval covers actor/org/realm/default revision/merge mode for its time window, including subsequent calls. Editing the default or merge policy invalidates it. A preparation freezes one action/target/offer for five minutes; acceptance still has the original ringing deadline. No command approves context automatically.

## Calls and delegation

Supports **carbon–carbon, carbon–silicon, silicon–silicon, and groups with any number of participants in any mix**. Use `call init` to start and `call invite` to add people. There is no fixed participant-count limit; each silicon can participate in only one call at a time.

| Command | Action / API |
|---|---|
| `ring call init ACTOR` | Prepare a silicon's context, then `calls.init`. Flags: `--context TEXT`, `--context-mode MODE`, `--start TEXT`, carbon `--device ID`, `--live`. |
| `ring call accept RINGID` | `calls.accept` after context checks. Same flags as init, plus `--invitation ID`. Atomically claims one device and silences ringing elsewhere. |
| `ring call decline RINGID [--invitation ID] [--reason TEXT / --give-no-reason]` | `calls.decline`. Silicon must supply a reason or explicit opt-out; carbon may omit both. Eligible caller gets voicemail. |
| `ring call cut RINGID` | `calls.cut`: leave as yourself, or cancel your unanswered outgoing attempt. Remaining participants continue while a conversation can remain. |
| `ring call invite ACTOR RINGID` | `calls.invite`: offer another actor a place in the existing conference; show them the current roster. |
| `ring call silence RINGID [--invitation ID]` | `calls.silence`: stop recipient ringing everywhere without declining/extending the offer. |
| `ring call show RINGID` | Participants, invitations, timings, outcomes and media location: `calls.get`. |
| `ring call history [--state STATE] [--direction incoming/outgoing/all] [--actor ACTOR] [--since TIME] [--until TIME]` | `calls.list`; states: `ringing`, `connecting`, `active`, `voicemail`, `ended`. |
| `ring call transcript RINGID [--kind KIND] [--after-seq N]` | Authorized transcript/lifecycle/delegation history: `transcript.list`. Cursor and `--after-seq` are alternatives. |
| `ring call recording RINGID [--play / --audio-out PATH] [--overwrite]` | Recording status/coverage: `recordings.get`; ready audio via `assets.get`. Downloads only authorized participation intervals. |
| `ring call watch RINGID [--after-seq N]` | `events.subscribe`; replay cursor is an event sequence, not a transcript sequence. Ctrl-C unsubscribes without hanging up. |
| `ring call handoff RINGID --to-device ID` | Carbon-only; rejects silicon callers. Moves audio to an owned ready device through `calls.handoff`; no representative restart or join/leave transcript entry. |
| `ring call mute RINGID` / `ring call unmute RINGID` | Carbon microphone control: daemon resolves owned stream and calls `media.state`. Incoming audio continues. |
| `ring send RINGID thinking/commentary TEXT [--delegationid ID]` | Silicon-only `representative.send` to its own active representative. `--text-file` replaces TEXT; `--delegation-id` is an alias. Omission sends `delegation_id:null`. |
| `ring delegation ls RINGID [--status open/closed]` / `ring delegation show ID` | Own requests, context and replies: `delegations.list/get`. Multiple answers can share one delegation ID. |

Context and `--start` are silicon-only. Start is the first utterance after connection, not during ringing. A silicon has one reserved/active call across orgs/devices; outgoing ringing reserves its slot. A second device cannot steal an accepted call; carbons use handoff to switch devices. With one participant remaining, the room ends unless a pending invitation can connect before its existing deadline.

The server records the full connected call, including carbon-only calls and handoffs. Completed recordings become available after finalization; gaps/failures/purging are explicit. Transcript and recording access follow participation permissions. Thinking blocks and private delegation content are visible only to their owning silicon.

## Voicemail

| Command | Action / API |
|---|---|
| `ring voicemail ls [--all] [--actor ACTOR] [--since TIME] [--until TIME]` | Unread inbox by default: `voicemail.list`; `--all` sends `unread:false` for read and unread. |
| `ring voicemail show ID [--play / --audio-out PATH] [--overwrite]` | Metadata/transcript via `voicemail.get`; audio via `assets.get`. Does not mark read. |
| `ring voicemail read ID` / `ring voicemail unread ID` | `voicemail.mark {read:true/false}`. |
| `ring voicemail delete ID` | Delete the recipient's inbox message: `voicemail.delete`; not the enclosing call history. |
| `ring voicemail leave RINGID [--invitation ID]` | Eligible outbound voicemail only. Silicon: `--text TEXT` or `--text-file PATH`. Carbon: `--record [--duration DURATION]` or `--audio-file PATH`. |
| `ring voicemail begin RINGID --format text/audio [--invitation ID]` | Low-level draft: `voicemail.begin`. |
| `ring voicemail send ID --text TEXT` / `ring voicemail commit ID` / `ring voicemail abort ID` | `voicemail.send/commit/abort`. Send also supports `--text-file`; text fills once. Different text requires a new draft. |
| `ring voicemail greeting show [--when busy/declined/timeout]` | Effective reason-specific greetings: `config.get`. |
| `ring voicemail greeting set --when REASON` | `--text/--text-file`, or carbon `--audio-file/--record [--duration]`; upload if needed, then `config.set`. |
| `ring voicemail greeting reset --when REASON` | Restore built-in greeting through `config.reset`. |

Silicon greetings and messages use its selected voice through OpenAI TTS. Carbon recordings use Deepgram STT. Recording begins after the greeting/beep; Enter finishes, Ctrl-C aborts, `--duration` enables unattended recording. A carbon recording private voicemail while in a conference pauses/restores its conference microphone. Private audio never leaks into the room.

## What happens under the hood

These are shared library/daemon operations, never recursive shell calls to `ring`.

| Operation | Sequence |
|---|---|
| Authenticated command | Start daemon if needed → `protocol.hello` → `auth.resume` → requested operation. Login uses `auth.login`. |
| Silicon init/accept | `calls.prepare` → show context → stop if approval missing → otherwise `calls.init/accept`. Explicit `context.approve` is separate; repeat the original command afterward. |
| Carbon init/accept | Check device → call mutation → daemon `media.attach` when connecting. CLI exits; daemon retains audio. |
| `--live` / watch | Subscribe/replay authorized events → render → unsubscribe on exit. Never calls `calls.cut`. |
| Carbon-only handoff | Destination `media.attach` in standby → ready → `calls.handoff` switches authority → old stream detaches. Failure before switch preserves old audio. |
| Silicon voicemail | `voicemail.begin` → `voicemail.send` → wait for synthesis via events/`voicemail.get` → `voicemail.commit`. |
| Carbon voicemail | Begin → attach private media → record/stream → `media.detach {last_seq}` → verify persisted recording → commit. Missing chunks require abort/re-record. |
| Photo/audio greeting | `assets.begin` → WS upload → `assets.complete` → `profile.update` or `config.set`. Failed upload leaves the previous setting intact. |
| Recording/voicemail playback | `recordings.get` or `voicemail.get` → authorized `assets.get` → local playback/file. Existing output files require `--overwrite`. |

## Runtime and support

| Command | Action |
|---|---|
| `ring notifications status` / `ring notifications retry [ID]` | `notifications.status/retry`; silicon Ring-to-Ting readiness/publication retries. Carbon Ting integration is not applicable. |
| `ring daemon start/status/stop` | Local runtime management. Stop detaches local carbon media; it does not cut server-owned silicon calls or stop Ting. |
| `ring daemon logs [--since TIME] [--limit N] [--follow]` | Redacted local logs; default last 100, optional JSON Lines. |
| `ring update check/apply` | `release.info`, then verified installation when applying. Defer disruptive daemon restart while carrying audio. |
| `ring doctor [--output PATH] [--overwrite]` | Local checks plus app/auth/notification status; redacted report and concrete recovery steps. No automatic submission. |
| `ring bug report --title TEXT --description TEXT [--reproduction TEXT] [--expected TEXT] [--actual TEXT] [--pr URL]` | `bugs.submit`; `--description-file` replaces description. Basic report allowed; reproduction and PR encouraged. No automatic private-log upload. |

Product defaults: ringing 60 seconds; ringback cycle 3 seconds; Ting transcript batches every 10 seconds; delegation context last 30 seconds; daemon update checks/installations hourly. Ting owns shared delivery/retries and interpreter webhooks. Notifications continue after CLI exit, including incoming calls, delegations, voicemail offers and received voicemail.

Rust backend and reusable Rust client serve the CLI/daemon; platform apps have native integration with SolidJS frontend. Planned hosts: `https://ring.teamofsilicons.com` and `wss://backend.ring.teamofsilicons.com/ws`. AWS hosts the backend; select the smallest server meeting 10–15 concurrent calls through load tests. S3 stores assets/recordings, with org BYO buckets supported. OpenAI handles Live/TTS; Deepgram handles voicemail STT only, never voice generation. Exact model IDs are pinned during integration.

Space Station telemetry defaults on; `telemetry.enabled:false` opts out at actor/org/local scope as applicable. Secrets and conversation bodies are excluded from ordinary diagnostics. `--test` uses isolated identities/data/notifications and the same paths; protected `SILICON_RING_TEST_APP_SECRET_FILE` or `SILICON_RING_TEST_APP_SECRET` supplies harness credentials, while IAM test tokens identify actors. Protocol major 1 is negotiated; breaking changes require a new major, published compatibility/deprecation information and consumer contract checks.

The installer sets up the CLI/daemon and updates, not authentication. Replace the unassigned installer URL before publishing:

```sh
curl -fsSL '<PUBLISHED_RING_INSTALL_SCRIPT_URL>' | sh
```
