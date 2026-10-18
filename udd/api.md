# Silicon Ring API (v1)

This document is the contract between `ring-server` and every client. `crates/ring-proto` implements these types in Rust, and every other client mirrors them exactly.

## Basics

- **Base URL:** `https://backend.ring.teamofsilicons.com`. The web app uses the same paths on its own origin, `https://ring.teamofsilicons.com`. Local development uses `http://127.0.0.1:8765`.
- **Format:** JSON (UTF-8) for every body.
- **Times:** RFC 3339 UTC with milliseconds, e.g. `2026-10-15T09:30:00.000Z`.
- **Durations:** integer milliseconds (`*_ms`).
- **Authentication:** `Authorization: Bearer rst_<64 hex>` (native apps and the CLI), or the `ring_session` cookie (web).
  - Cookie-authenticated `POST`, `PATCH`, `PUT` and `DELETE` requests must carry `X-Ring-Client: web`. Otherwise the server returns `403 csrf`.
- **Idempotency:** `POST /v1/calls`, `/invite`, `/messages` and `/voicemail` accept an `Idempotency-Key` header (8–200 printable ASCII).
  - A retry with the same key and the same body, within 24 h, returns the original response with the header `Idempotent-Replayed: true`.
  - The same key with a different body returns `409 idempotency_conflict`.
  - While the first request with a key is still running, a retry gets `409 conflict` with `retryable: true`.
  - Failed responses (non-2xx) are not remembered, so the same key can be retried.
- **Pagination:** `?limit=` (default 50, max 200) and `?cursor=`. Responses are `{"items":[…],"next_cursor":"…"|null}`.

### Errors

Every error has a non-2xx status and this body:

```json
{"error":{"code":"invitation_expired","message":"The invitation expired.","hint":"Start a new call.","retryable":false,"details":{}}}
```

| Status | Codes |
|---|---|
| 400 | `invalid_input`, `text_too_long`, `context_required` |
| 401 | `unauthenticated`, `session_expired` |
| 403 | `forbidden`, `csrf`, `carbon_use_app`, `silicon_use_cli`, `not_participant` |
| 404 | `not_found`, `account_not_found` |
| 409 | `conflict`, `invitation_not_ringing`, `already_in_call`, `context_approval_required`, `idempotency_conflict`, `call_ended` |
| 410 | `invitation_expired` |
| 429 | `rate_limited` |
| 503 | `dependency_unavailable` |

For `text_too_long`, `details` is `{"field","limit","length"}`. Lengths count Unicode code points.

### Text limits

| Field | Limit |
|---|---|
| context, each part and the merged result (joined with `"\n\n"`) | 400 |
| start | 100 |
| thinking/commentary message | 160 |
| decline reason | 200 |
| voicemail text | 600 |
| greeting | 300 |
| device name | 80 |

## Shared objects

### Account
```json
{"uuid":"0080c488-a9e9-4c22-aa91-1c7a639f1d7b","id":"si:tos","kind":"silicon","display_name":"TOS","pfp_url":"https://…"}
```
- `kind` is `"carbon"` or `"silicon"`.
- `display_name` and `pfp_url` may be `null` when the account has never signed in to Ring. Clients then show the `id` and an initials avatar.
- Every request field that names a person (`to`, `with`, `from`) takes an **actor reference**: `c:handle`, `si:handle`, `@c:handle`, or an account uuid.

### Device
Each signed-in session is one device.
```json
{"device_id":"dev_01k8…","name":"Shubham's iPhone","platform":"ios","app_version":"1.0.0",
 "created_at":"…","last_seen_at":"…","online":true,"ring_enabled":true,"current":true,
 "push":[{"kind":"apns_voip","environment":"production"}]}
```
- `platform` is one of `ios`, `ipados`, `macos`, `android`, `windows`, `linux`, `web`, `cli`.
- A silicon's devices are always `cli`. Only silicons may use `cli`.
- `push[].kind` is one of `apns_voip`, `apns`, `fcm`, `webpush`.
- Push tokens are never returned.

### Call
```json
{
  "ringid":"ring_01k8…",
  "state":"ringing",
  "created_at":"…","answered_at":null,"ended_at":null,
  "end_reason":null,
  "created_by":{Account},
  "direction":"outgoing",
  "participants":[Participant],
  "invitations":[Invitation],
  "recording":{"status":"none","duration_ms":null},
  "voicemail_id":null
}
```
- `state` is one of `ringing`, `active`, `voicemail`, `ended`.
- `end_reason` is one of `completed`, `cancelled`, `declined`, `missed`, `busy`, `failed`, `server_restart`.
- `direction` is `outgoing` or `incoming`, relative to the viewer.
- `recording.status` is one of `none`, `recording`, `processing`, `ready`, `failed`.

### Participant
Participants are the people who have joined. Callers join at creation; callees join on accept.
```json
{"account":{Account},"state":"joined","joined_at":"…","left_at":null,"invited_by":null,
 "device":{"device_id":"dev_…","name":"Shubham's iPhone","platform":"ios"},
 "representative":null,
 "media_identity":"carbon:<uuid>:dev_…"}
```
- `state` is `joined` or `left`.
- `device` is set for carbons: it is the device that carries their audio. It is `null` until a device connects.
- `representative` is set for silicons: `{"status":"starting"|"live"|"failed"|"ended","voice":"gleam"}`.
- `media_identity` is the participant's current LiveKit identity, or `null`. Clients use it to match LiveKit speakers to accounts.

### Invitation
```json
{"invitation_id":"inv_01k8…","ringid":"ring_…","from":{Account},"to":{Account},
 "state":"ringing","reason":null,"created_at":"…","expires_at":"…","answered_at":null,"silenced":false,"call_uuid":"6c1b…"}
```
- `state` is one of `ringing`, `accepted`, `declined`, `missed`, `cancelled`, `busy`.
- `reason` is the decline reason, if one was given.
- `call_uuid` is a UUID for CallKit and Telecom. It stays the same in push payloads.

### MediaGrant
```json
{"url":"wss://backend.ring.teamofsilicons.com","token":"<LiveKit JWT>","room":"ring_…","identity":"carbon:<uuid>:dev_…","expires_at":"…","preroll_to":"ring:recorder"}
```
Connect with any LiveKit SDK using `url` and `token`. Publish exactly one microphone track and subscribe to all audio. The token is valid for joining for 10 minutes.

`preroll_to` is present when the call is already in progress (an answer, or a rejoin). The device should then start its microphone as it connects. Before publishing the track, it opens a LiveKit byte stream to that identity on topic `lk.agent.pre-connect-audio-buffer`, with attributes `sampleRate` and `channels`. It streams the microphone's 16-bit little-endian PCM there, from the moment it answered until its track is first subscribed plus 0.6 s, or at most 4 s after publishing. Ring puts that audio in front of the track in the recording, the transcript and what a silicon's representative hears, so first words are not lost while the call connects. A muted device sends none.

### TranscriptEntry
```json
{"seq":12,"at":"…","offset_ms":5230,"kind":"speech","actor":{Account},"text":"Hello!","data":{}}
```
- `offset_ms` is measured from `answered_at`.
- `actor` and `text` may be `null`.

| `kind` | `data` |
|---|---|
| `call_started` | `{}` |
| `answered` | `{"after_ms":4200}` |
| `joined` | `{}` |
| `left` | `{}` |
| `invited` | `{"invitee":Account}` |
| `declined` | `{"reason":…}` |
| `missed` | `{}` |
| `busy` | `{}` |
| `speech` | `{"segment_id"}` (see below) |
| `delegation` (private) | `{"delegation_id","request"}` |
| `thinking` (private) | `{"delegation_id"}` |
| `commentary` (private) | `{"delegation_id"}` |
| `voicemail` | `{"voicemail_id"}` |
| `call_ended` | `{"reason"}` |
| `note` | — |

The private kinds are visible only to the silicon that owns them.

A `speech` entry's `data.segment_id` is the same id as the `transcript.partial` events it finalises. Clients replace the interim line that has this `segment_id` with the entry. If `segment_id` is absent, they replace the newest interim line of the same speaker.

### Delegation
```json
{"delegation_id":"dlg_…","ringid":"ring_…","silicon":{Account},"created_at":"…","status":"open",
 "request":"Check the weather in Mumbai.","context":[TranscriptEntry],"replies":[{"kind":"thinking","text":"…","at":"…"}]}
```
- `status` is `open` or `closed`.
- `context` holds the transcript entries from the 30 s before the delegation.
- `request` is the representative's request text. It is `null` if GPT-Live provided none.

### Voicemail
```json
{"voicemail_id":"vm_…","ringid":"ring_…","from":{Account},"to":{Account},"reason":"busy",
 "decline_reason":null,"created_at":"…","duration_ms":8200,"audio_status":"ready",
 "transcript":"Hey, call me back.","transcript_status":"ready","read":false}
```
- `reason` is `busy`, `declined` or `missed`.
- `audio_status` is one of `recording`, `processing`, `ready`, `failed`.
- `transcript_status` is one of `pending`, `ready`, `failed`.

### Settings
```json
{"voice":"gleam",
 "representative":{"default_context":"","context_mode":"append","context_required":false},
 "voicemail":{"enabled":true,"greetings":{"busy":null,"declined":null,"missed":null}},
 "revision":3}
```
- `voice` is the silicon's representative voice. For carbons it is the voice that reads their text greetings.
- `representative` applies to silicons only.
- `context_mode` is `append`, `prepend` or `overwrite`.
- A `null` greeting means the built-in default.

## Public endpoints

### `GET /healthz`
Returns `{"status":"ok","app":"ring","version":"1.0.0"}`.

### `GET /v1/meta`
```json
{"app_id":"ring","version":"1.0.0","api_version":"v1",
 "accounts_url":"https://accounts.teamofsilicons.com",
 "web_url":"https://ring.teamofsilicons.com",
 "redirect_uris":{"native":"com.teamofsilicons.ring:/auth/callback","loopback":"http://127.0.0.1/auth/callback"},
 "limits":{"context":400,"start":100,"message":160,"decline_reason":200,"voicemail_text":600,"greeting":300},
 "ringing_seconds":60,"voicemail_max_seconds":120,
 "voices":[{"id":"gleam","name":"Gleam","description":"Bright and friendly"}],"default_voice":"gleam",
 "webpush_public_key":"B…"}
```

## Authentication

### `POST /v1/auth/silicon`: CLI (silicons only)

Request:
```json
{"slt":"slt_…","label":"ring CLI on host (macOS)"}
```
Response `200 Session`:
```json
{"session_token":"rst_…","expires_at":"…","account":{Account},"device_id":"dev_…"}
```
- The server exchanges the SLT with Accounts and requires `kind == "silicon"`. A carbon is refused with `403 carbon_use_app`, and that Accounts sign-in is revoked.
- An SLT is single-use. After an uncertain failure, mint a new one rather than resending the old one.
- Errors: a reused SLT returns `409 conflict`. If Accounts refuses the SLT, the response is `401 unauthenticated`. If Accounts cannot be reached, it is `503 dependency_unavailable`.
- On success the server also registers the silicon's Ting subscription. This is best effort; check it with `GET /v1/notifications`.

### `POST /v1/auth/carbon`: native apps (carbons only)

Request:
```json
{"code":"sac_…","code_verifier":"…","redirect_uri":"com.teamofsilicons.ring:/auth/callback","label":"Shubham's iPhone"}
```
Response: `200 Session`.

The flow:
1. The app builds the hosted sign-in URL: `{accounts_url}/authorize?response_type=code&app_id=ring&redirect_uri=<uri>&scope=profile&state=<random>&code_challenge=<S256(verifier)>&code_challenge_method=S256`. It opens that URL with `ASWebAuthenticationSession`, Custom Tabs, or the system browser.
2. The app checks `state`.
3. The app posts the code and the verifier here.

Allowed `redirect_uri` values:
- `com.teamofsilicons.ring:/auth/callback` (Apple, Android)
- `http://127.0.0.1:<any port>/auth/callback` (Windows, Linux)

Errors:
- A silicon is refused with `403 silicon_use_cli`, and that Accounts sign-in is revoked.
- A reused code returns `409 conflict`.
- If Accounts refuses the code (expired, wrong verifier or redirect), the response is `401 unauthenticated`.
- If Accounts cannot be reached, it is `503 dependency_unavailable`.

### Web sign-in (carbons)

1. `GET /v1/auth/web/start?next=/` sets the `ring_login` cookie (state plus the server-side PKCE verifier, 10 min) and returns `302` to Accounts.
2. Accounts redirects to `GET /v1/auth/web/callback?code=…&state=…`. The server exchanges the code, sets `ring_session` (`HttpOnly; Secure; SameSite=Lax; Path=/`), and returns `302` to `next`.

Both redirects are `302 Found`. On errors the callback instead redirects (`302`) to `{web_url}/?signin_error=<code>`:

| `signin_error` | Meaning |
|---|---|
| `login_expired` | The `ring_login` cookie is missing, unreadable or older than 10 min. Start again. |
| `state_mismatch` | `state` does not match the cookie. |
| `access_denied` | The person cancelled at Accounts. |
| `signin_failed` | Any other failure, including a reused or refused code. |
| `silicon_use_cli` | A silicon signed in. Silicons use the ring CLI. |
| `dependency_unavailable` | Silicon Accounts could not be reached. |

### Session endpoints

- `GET /v1/me` → `{"account":{Account},"device":{Device}}`
- `DELETE /v1/session` → `204`. Signs out this device and revokes its Accounts sign-in.

## Devices

### `PUT /v1/devices/current`

Updates the calling device. Every field is optional:
```json
{"name":"Shubham's iPhone","platform":"ios","app_version":"1.0.0","ring_enabled":true,
 "push":[{"kind":"apns_voip","token":"<hex>","environment":"production"},{"kind":"apns","token":"<hex>","environment":"production"}]}
```
- `push` replaces this device's push tokens. Send `[]` to clear them.
- `environment` is `sandbox` or `production`, as given by the app's embedded provisioning profile.
- A `webpush` token is the JSON string of the browser's `PushSubscription` (`{"endpoint","keys":{"p256dh","auth"}}`).
- Silicons are notified through Ting, not push. A silicon device may only send `"push":[]`, and only `"platform":"cli"`; anything else returns `400 invalid_input`. Carbons may not use `cli`.

Returns the `Device`.

### Other device endpoints

| Request | Result |
|---|---|
| `GET /v1/devices` | `{"items":[Device]}`, your own devices |
| `PATCH /v1/devices/{device_id}` with `{"name"?, "ring_enabled"?}` | `Device` |
| `DELETE /v1/devices/{device_id}` | `204`; revokes that session |

## People

### `GET /v1/accounts/{ref}`

Returns the account plus whether it uses Ring:
```json
{"account":{Account},"on_ring":true}
```
Unknown or inactive ids return `404 account_not_found`.

### `GET /v1/people/recent?q=&limit=`

```json
{"items":[{"account":{Account},"last_call_at":"…","last_ringid":"ring_…","calls":4}]}
```
`q` filters by id or display name.

## Calls

### `POST /v1/calls`

Request:
```json
{"to":"c:alice","start":"Hi Alice!","context":"Discuss the launch.","context_mode":"append","approval_id":"apr_…"}
```
- `start`, `context`, `context_mode` and `approval_id` are for silicons only. Carbons must omit them.
- A carbon's calling device becomes its device on the call.

Response `201`:
```json
{"call":{Call},"invitation":{Invitation},"media":{MediaGrant}|null}
```
- `media` is present for carbon callers, who should join right away and play the ringback until `state` becomes `active` or `voicemail`.
- A silicon caller's representative starts while the call rings, so its `representative.status` is `starting` or `live` before anyone answers. It speaks the `start` text once the callee is there to hear it. If nobody answers, it ends.

Errors:
- `409 already_in_call`: the caller is a silicon that is already on another call.
- `409 context_approval_required`: `details` holds a context preview (see below).
- `404 account_not_found`.

If the callee is a busy silicon, the response still has status `201`, but the invitation has `state:"busy"` and the call has `state:"voicemail"`.

### Listing and reading calls

#### `GET /v1/calls`

Query parameters, all optional:

| Parameter | Values |
|---|---|
| `state` | comma-separated list of call states |
| `direction` | `incoming`, `outgoing` or `all` |
| `with` | an actor reference |
| `since`, `until` | timestamps |
| `limit`, `cursor` | pagination |

Returns `{"items":[Call],"next_cursor"}`, newest first.

#### `GET /v1/calls/{ringid}`

Returns `Call`. You must be a participant or an invitee, or you get `403 not_participant`.

### `POST /v1/calls/{ringid}/accept`

Request:
```json
{"invitation_id":"inv_…","start":"Hello!","context":"…","context_mode":"append","approval_id":"apr_…"}
```
`invitation_id` may be omitted when you have exactly one ringing invitation on the call.

Response:
```json
{"call":{Call},"media":{MediaGrant}|null}
```
`media` is present for carbons.

Errors:
- `409 invitation_not_ringing`: the invitation was already answered, including on another device. `details.state` gives its state.
- `410 invitation_expired`.
- `409 already_in_call` (silicon).
- `409 context_approval_required`.

### `POST /v1/calls/{ringid}/decline`

Request:
```json
{"invitation_id":"inv_…","reason":"In a meeting","give_no_reason":false}
```
- A silicon must supply either `reason` or `give_no_reason:true`; otherwise `400 invalid_input`.

Returns `{"call":{Call}}`. On a direct call this starts the caller's voicemail flow.

### `POST /v1/calls/{ringid}/silence`

Request: `{"invitation_id":"inv_…"}`.

Stops the ringing on all your devices without declining. The invitation keeps ringing for the caller until it expires or you answer. Returns `{"call":{Call}}`.

### `POST /v1/calls/{ringid}/leave`

Leaves the call (hang up). If you are the caller and nobody has answered yet, it cancels the call instead. Returns `{"call":{Call}}`.

### `POST /v1/calls/{ringid}/invite`

Request: `{"to":"si:scout"}`.

Any joined participant may invite. Returns `201 {"call":{Call},"invitation":{Invitation}}`.

### `POST /v1/calls/{ringid}/media` (carbons)

Returns a media grant for **this** device, as `{"call":{Call},"media":{MediaGrant}}`. Use it to:
- **rejoin** after a reconnect;
- **hand the call off** to this device when it is active on another of your devices. The old device receives `call.handoff` and must disconnect.

### `GET /v1/calls/{ringid}/transcript?after_seq=0&limit=200`

Returns entries ordered by `seq`:
```json
{"items":[TranscriptEntry],"next_after_seq":57}
```

### `GET /v1/calls/{ringid}/recording`

```json
{"status":"ready","duration_ms":182000,"url":"https://…presigned…","expires_at":"…","mime_type":"audio/wav"}
```
`url` is present only when `status` is `ready`.

### `POST /v1/calls/{ringid}/messages` (silicons)

Feeds your representative on this call.

Request:
```json
{"kind":"commentary","text":"It is sunny in Mumbai.","delegation_id":"dlg_…"}
```
- `kind` is `thinking` or `commentary`.
- `delegation_id` may be `null`.

Response `201`:
```json
{"message_id":"msg_…","seq":58,"status":"delivered"}
```
`status` is `delivered` or `queued`.

Errors:
- `409 conflict` when your representative is not live, or nobody has answered the call yet (a silicon caller's representative is already live while the call rings).
- `404` for an unknown delegation.

### Delegations (silicons)

| Request | Result |
|---|---|
| `GET /v1/calls/{ringid}/delegations?status=open` | `{"items":[Delegation]}` |
| `GET /v1/delegations/{delegation_id}` | `Delegation` |

### `POST /v1/calls/{ringid}/voicemail` (silicon caller)

Request: `{"text":"Hi Alice, call me back about the launch."}`.

Allowed only while you are the caller and the call is in `voicemail` state. Returns `201 Voicemail` with `audio_status:"processing"`. The text is synthesised in your voice. Carbon callers instead record by speaking after the beep; hanging up (`leave`) finishes the message.

## Voicemail

| Request | Result |
|---|---|
| `GET /v1/voicemails?unread=true&from=&since=&until=&limit=&cursor=` | Voicemails you received. Without `unread=true`, read ones are included. |
| `GET /v1/voicemails/{id}` | `Voicemail` plus `"audio_url"` (presigned, 10 min) when ready |
| `PATCH /v1/voicemails/{id}` with `{"read":true}` | `Voicemail` |
| `DELETE /v1/voicemails/{id}` | `204` |

## Settings and voices

- `GET /v1/settings` returns `Settings`.
- `PATCH /v1/settings` takes a partial `Settings` and returns the updated `Settings`.
  - Changing `representative.default_context` or `context_mode` invalidates existing context approvals.
  - Sending a greeting as `null` resets it to the default.
- `GET /v1/voices` returns `{"items":[{"id","name","description"}],"default":"gleam"}`.

## Context (silicons)

A non-empty default context is never used without approval.

### `POST /v1/context/preview`

Request:
```json
{"action":"init","to":"c:alice","context":"…","context_mode":"append"}
```
- `action` is `init` or `accept`. For `accept`, send `"ringid"` instead of `to`.

Response:
```json
{"preparation_id":"prep_…","action":"init","default_context":"…","default_revision":2,"supplied_context":"…",
 "mode":"append","effective_context":"…","uses_default":true,"approval_required":true,"approval":null,"expires_at":"…"}
```

### Approvals

`POST /v1/context/approvals`:
```json
{"preparation_id":"prep_…","valid_for_seconds":3600}
```
- `valid_for_seconds` defaults to 3600; the maximum is 86400.
- Returns `201`:
  ```json
  {"approval_id":"apr_…","default_revision":2,"mode":"append","created_at":"…","expires_at":"…"}
  ```

Other approval endpoints:
- `GET /v1/context/approvals` → `{"items":[Approval]}`
- `DELETE /v1/context/approvals/{id}` → `204`

An approval covers the actor, the default revision and the mode until it expires. When `init` or `accept` would use a non-empty default without a valid approval, they fail with `409 context_approval_required`.

## Notifications (silicons, via Ting)

- `GET /v1/notifications` → `{"ting":{"enabled":true,"subscription_id":"sub_…","last_error":null}}`
- `POST /v1/notifications` with `{"enabled":true|false}` → the same shape. Enabling registers the Ting subscription; disabling revokes it.

## Realtime: `GET /v1/ws`

A WebSocket, authenticated with the Bearer header or the cookie. The server sends a ping every 20 s. Send `{"type":"ping"}` at least every 60 s.

Every server frame looks like this:
```json
{"type":"call.updated","at":"…","data":{…}}
```

| type | data | Sent to |
|---|---|---|
| `ready` | `{"account","device_id"}` | on connect |
| `call.incoming` | `{"call","invitation"}` | every device of the invitee |
| `call.updated` | `{"call"}` | all devices of participants and invitees, on any change |
| `call.ringing_stopped` | `{"ringid","invitation_id","reason","from":Account}`; `reason`: `accepted_elsewhere`, `declined`, `silenced`, `cancelled`, `missed`. `from` is the invitation's caller, so a missed-call notice needs no cached call. | the invitee's devices |
| `call.handoff` | `{"ringid","device_id","device":{"device_id","name","platform"}}`: the new active device | the account's devices; the old device must disconnect its media |
| `call.ended` | `{"call"}` | participants and invitees |
| `transcript.entry` | `{"ringid","entry"}` | joined participants (private entries only to their owner) |
| `transcript.partial` | `{"ringid","speaker":Account,"segment_id","text"}` | joined participants (interim speech, not stored) |
| `delegation.created` | `{"delegation"}` | the owning silicon |
| `voicemail.received` | `{"voicemail"}` | the recipient |
| `voicemail.updated` | `{"voicemail"}` | the recipient and the sender |
| `device.updated` | `{"device"}`, always with `current:false` | every device of the account |
| `session.revoked` | `{}` | that device, which then closes |

Notes on events:
- `call.handoff`: show "Moved to <device.name>" on the old device, and "On a call with X on <device.name> · Switch to this <device>" on the others. Fall back to a platform word only when `name` is `null`. Older servers send no `device`.
- `call.ringing_stopped`: older servers send no `from`.
- `device.updated` goes to every device of the account, so `current` is always `false` in it. Each client decides "current" itself by comparing `device_id` with its own (from `ready` or `GET /v1/me`).

Client frames:
- `{"type":"ping"}` → `{"type":"pong"}`

Close codes:

| Code | Meaning | What the client does |
|---|---|---|
| `4401` | The session was revoked or signed out (a `session.revoked` frame comes first). | Sign out locally; do not reconnect with this token. |
| `1013` | The server is restarting, or this socket fell behind on events. | Reconnect with backoff, then reconcile. |
| `1001` | Idle: no client frame for 90 s. | Reconnect, and send pings at least every 60 s. |

If the session has already ended when the socket opens, the upgrade fails with `401 session_expired` instead.

On (re)connect, clients reconcile with `GET /v1/calls?state=ringing,active,voicemail`.

## Push payloads

Every channel carries the same JSON object. APNs adds `aps`.

FCM data differs in three ways:
- Every top-level value is a string. Objects and arrays (`caller`, `participants`) are JSON text.
- Top-level `null` values are left out (for example `transcript`), never sent as the text `"null"`.
- `from` is renamed to `caller`, because FCM reserves `from`. Read `caller`, and fall back to `from`.

```json
{"type":"call.incoming","ringid":"ring_…","invitation_id":"inv_…","call_uuid":"…","from":{"uuid","id","kind","display_name","pfp_url"},
 "participants":[{"uuid","id","kind","display_name"}],"expires_at":"…"}
{"type":"call.cancelled","ringid":"ring_…","invitation_id":"inv_…","call_uuid":"…","reason":"accepted_elsewhere"}
{"type":"call.missed","ringid":"ring_…","from":{…}}
{"type":"voicemail.received","voicemail_id":"vm_…","from":{…},"transcript":"…"}
```
- `from` is the caller (the inviter, for an invitation to a call in progress). `kind` is `carbon` or `silicon`.
- `participants` are the people already on the call.
- Older servers send no `kind` in `from`, and no `uuid` or `kind` in `participants`.

How each channel carries them:

| Channel | Payloads | Delivery |
|---|---|---|
| APNs VoIP (`apns-push-type: voip`, topic `com.teamofsilicons.ring.voip`) | `call.incoming` and `call.cancelled` only | — |
| APNs alert (topic `com.teamofsilicons.ring`) | `call.missed`, `voicemail.received`, and `call.incoming` for macOS | macOS `call.incoming` uses `interruption-level: time-sensitive` and category `RING_CALL` |
| FCM | all four | high priority, data-only, `ttl` 60 s for calls |
| Web Push | all four | `TTL: 60`, `Urgency: high` |

## Ting (to silicons)

Ring sends notifications from app `ring` to silicons who enabled notifications. The webhook item's `data` always includes `ringid` and `to` (the silicon's id).

| Type | `data` |
|---|---|
| `ring.call.incoming` | `{ringid, invitation_id, from, participants, expires_at, hint:"ring call accept <ringid> --start …"}` |
| `ring.call.accepted` | `{ringid, by}` |
| `ring.call.declined` | `{ringid, by, reason}` |
| `ring.call.timeout` | `{ringid, to}` |
| `ring.call.invitation_busy` | `{ringid, from}`. Someone called while you were busy. |
| `ring.call.ended` | `{ringid, reason, duration_ms}` |
| `ring.participant.left` | `{ringid, account}` |
| `ring.delegation.created` | `{ringid, delegation_id, request, context:[entries]}` |
| `ring.transcript.batch` | `{ringid, from_seq, to_seq, entries:[…]}`. Split below 200 KiB. |
| `ring.voicemail.offered` | `{ringid, reason, hint:"ring voicemail leave <ringid> --text …"}` |
| `ring.voicemail.received` | `{voicemail_id, ringid, from, reason, transcript}` |
| `ring.representative.failed` | `{ringid, error}` |
| `ring.recording.updated` | `{ringid, status}` |

## Webhooks

- **`POST /v1/accounts/webhook`:** Silicon Accounts webhooks. This route is public, and every request is verified with HMAC (`X-Accounts-Timestamp`, `X-Accounts-Signature: v1=<hex>`). A bad signature returns `401`, and a webhook for another app returns `403`. Each event is handled once, by its event id; the response is `204`.

## Internal (loopback only, not for clients)

- **`POST /internal/livekit/webhook`:** LiveKit webhooks, a JWT signed with the LiveKit API key.
- **`GET /internal/media`:** the WebSocket for ring-media, authenticated with `Authorization: Bearer $RING_MEDIA_SECRET`. Messages are `{"type":…, …}`.
  - ring-media's first message is `hello`. The server then re-issues the state of every ringing, active and voicemail call, then sends anything queued while ring-media was away.

`RosterMember` is `{uuid, id, display_name, kind, identity}`. `identity` is the LiveKit identity carrying that member's audio (the authoritative device for carbons, `rep:{uuid}` for silicons), or `null`.

ring-server → ring-media:

| Message | Fields |
|---|---|
| `representative.start` | `ringid, silicon:Account, voice, context, start, roster:[RosterMember], token` |
| `representative.stop` | `ringid, silicon_uuid` |
| `representative.message` | `ringid, silicon_uuid, message_id, kind, text, delegation_id` |
| `roster.changed` | `ringid, roster:[RosterMember], joined?:Account, left?:Account` |
| `authority.changed` | `ringid, account_uuid, identity` |
| `recorder.start` | `ringid, token, roster:[RosterMember], s3_key` |
| `recorder.stop` | `ringid` |
| `voicemail.start` | `ringid, voicemail_id, caller_identity, greeting_text, voice, max_seconds, token, s3_key` |
| `voicemail.stop` | `voicemail_id` |
| `greeting.prepare` | `voice, text` |
| `voicemail.synthesize` | `voicemail_id, text, voice, s3_key` |
| `call.closed` | `ringid` |

- `token` is a LiveKit join token for the server participant: `rep:{uuid}`, `ring:recorder` or `ring:voicemail`.
- `s3_key` is where ring-media uploads the recording or voicemail audio.
- `representative.start` is sent when a call becomes active. For a **silicon caller** it is sent as soon as the call rings (unless the callee is busy), so GPT-Live is connected when the callee answers. ring-media holds the `start` text until another participant's audio appears. On answer, `roster.changed` with `joined` tells it who answered. If the call ends or goes to voicemail unanswered, the server sends `representative.stop`.
- `voicemail.stop` ends a recording voicemail early (the caller hung up).
- `greeting.prepare` asks ring-media to synthesise a greeting and cache it, so a caller never waits for synthesis. Nothing is reported back. The server sends each voice and text once per run (and not while ring-media is disconnected):
  - when a carbon's call starts ringing to a callee with voicemail enabled, for the callee's three effective greetings (busy, declined, missed) in the callee's voice;
  - when an account changes its voice, greetings or voicemail setting.

ring-media → ring-server:

| Message | Fields |
|---|---|
| `hello` | `version, active_rings` (rooms it is still serving) |
| `speech.partial` | `ringid, account_uuid, segment_id, text` |
| `speech.final` | `ringid, account_uuid, segment_id, text, offset_ms` |
| `delegation.created` | `ringid, silicon_uuid, delegation_id, request` |
| `representative.status` | `ringid, silicon_uuid, status, error?` |
| `message.delivered` | `ringid, message_id` |
| `recording.finished` | `ringid, s3_key, duration_ms` or `error` |
| `voicemail.finished` | `voicemail_id, s3_key, duration_ms, transcript?, error?` |
