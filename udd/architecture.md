# Silicon Ring architecture

## Components

```
                    ┌──────────────────── EC2 (us-west-1, Elastic IP 13.57.4.48) ────────────────────┐
 carbons ─HTTPS/WSS─►  Caddy :443                                                                     │
 (native/web)       │   ring.teamofsilicons.com          /v1/*,/v1/ws → ring-server :8765           │
                    │                                    /rtc*        → livekit-server :7880         │
                    │                                    *            → static web export            │
                    │   backend.ring.teamofsilicons.com  /rtc*        → livekit-server :7880         │
                    │                                    *            → ring-server :8765            │
 carbons ─UDP/TCP──►  livekit-server  7881/tcp (ICE-TCP), 7882-7892/udp (ICE), 3478/udp (TURN)        │
 silicons ─HTTPS───►│                                                                                 │
 (ring CLI)         │  ring-server (control plane, SQLite)  ◄──internal WS──►  ring-media             │
                    │    accounts · calls · invitations · push · Ting · history │  representatives    │
                    │                                                           │  recorder           │
                    │                                                           │  voicemail player   │
                    │                                                           │  Deepgram STT       │
                    └───────────────────────────────────────────────────────────┴─────────────────────┘
   ring-server → Silicon Accounts (sign-in, lookups, proofs), Ting (silicon notifications),
                 APNs (VoIP + alert), FCM v1, Web Push (VAPID)
   ring-media  → LiveKit (as server participants), OpenAI GPT-Live, Deepgram, S3
```

### ring-server (`crates/ring-server`, Rust, axum)
- Owns all state in SQLite, which runs in WAL mode under `RING_DATA_DIR`. There is one table per entity: accounts, sessions/devices, calls, participants, invitations, transcript entries, delegations, voicemails, settings, context approvals, outbox, idempotency and webhook events.
- Serves the HTTP API and the realtime WebSocket (`/v1/ws`). Events are fanned out through in-memory broadcast channels, one per account.
- Runs the call state machine. Each call has a per-call lock, and there is no global mutex.
- Mints LiveKit access tokens with `livekit-api`. Creates and deletes rooms and removes participants through LiveKit's RoomService on loopback. Receives LiveKit webhooks at `/internal/livekit/webhook`.
- Directs ring-media over a loopback WebSocket (`/internal/media`, shared secret). It starts and stops representatives, the recorder and voicemail sessions, and receives transcript, delegation, recording and voicemail results back.
- Integrates with Silicon Accounts as a confidential client using the `ring` app secret: SLT and code exchange, introspection, lookups, webhooks and proofs.
- Sends Ting notifications to silicons through a transactional outbox, using an App verification proof.
- Sends push notifications: APNs VoIP to iOS/iPadOS (CallKit), APNs alert to macOS, FCM high-priority data to Android, and Web Push to browsers. It also sends a cancel push when ringing stops.

### ring-media (`crates/ring-media`, Rust, LiveKit Rust SDK)
- A separate process, so that a libwebrtc failure can never drop signalling. It connects to ring-server's internal WebSocket and to LiveKit at `ws://127.0.0.1:7880`.
- **Representative.** There is one per silicon participant. It joins as `rep:{uuid}` and subscribes to every other audio track, keeping only the authoritative device of each carbon. It mixes them to 24 kHz mono for GPT-Live (`wss://api.openai.com/v1/live/sessions`) and publishes GPT-Live's audio as its own track.
  - Delegations, start text, roster changes ("X joined", "X left") and thinking/commentary messages are relayed through GPT-Live events.
  - A delegation with no commentary gets a "still checking" prompt at 12 s and a "will follow up" prompt at 35 s. Thinking restarts the clock. Commentary clears it for that delegation and every earlier one, as does commentary with no id or a new GPT-Live session. A prompt names what was asked. It waits until GPT-Live has been quiet for 1.5 s and the silicon's last commentary is at least 10 s old, because a prompt that lands mid-reply can displace an answer that has just arrived. At most one prompt goes out every 10 s, oldest delegation first; other delegations due within that window are covered by the same prompt.
- **Recorder.** A hidden participant `ring:recorder`. It subscribes to all audio, mixes it to a 48 kHz mono WAV, and uploads it to S3 when the call ends. 48 kHz is the rate Opus decodes at, so nothing a microphone sent is resampled away.
  - It joins with a separate subscriber connection. On LiveKit's single peer connection, every new track waits for the participant to renegotiate, and the SDK debounces that by 150 ms unless the participant may publish, which the recorder may not. With its own subscriber connection, LiveKit offers each new track itself and forwards it within about 20 ms of its first packet, instead of about 170 ms.
  - When a track goes away (or the call ends), the recorder logs its `audio reception`: codec, seconds and average bitrate received, packets received (50 a second without DTX) and lost, network jitter, libwebrtc's jitter-buffer delay, the share of audio it concealed, how much it time-stretched, and the mixer's own `late_ms`/`trimmed_ms`/`dropped_ms`. Concealment and stretching happen before the mixer, so this tells a lossy network or sender apart from a struggling server. Voicemail sessions log the same.
- **STT.** Each carbon's authoritative track is streamed to Deepgram live (`nova-3`, `language=multi`) as the recorder's own 48 kHz linear PCM. Both Deepgram requests (live and prerecorded) send `mip_opt_out=true`, so Deepgram does not keep call audio for model improvement. Speakers are attributed exactly, one track per person. A silicon's speech text comes from its representative's output transcript.
- **Pre-roll.** A track reaches ring-media only after the client has connected, published it, and LiveKit has offered it to each subscriber. That takes 1 to 3 s after a phone answers: production showed 0.8 to 2.7 s between an iPhone's connection coming up and its first packet. Whatever the carbon says meanwhile, typically their first words, never arrives as RTP. Clients therefore start the microphone when they connect and stream what it hears to `MediaGrant.preroll_to` (the recorder). This uses LiveKit's pre-connect audio format: a byte stream on `lk.agent.pre-connect-audio-buffer` with `sampleRate` and `channels` attributes, carrying 16-bit little-endian PCM at 24 kHz.
  - The client opens the stream before publishing, keeps streaming until its track is first subscribed plus 0.6 s, and stops 4 s after publishing in any case (LiveKit does not tell a publisher about the hidden recorder).
  - The recorder files each pre-roll for every session of the call (`preroll::Prerolls`). An unnamed one belongs to its sender's next new track; one naming a `trackId`, as LiveKit's own SDKs send it, belongs to that track. Only carbons' PCM is accepted, at most 15 s of it.
  - A session holds a new track back from its mix while the track's pre-roll is arriving, or while the track carries LiveKit's pre-connect flag. The hold lasts at most 4 s for the recorder and 2 s for a representative. The session matches the start of the live audio inside the pre-roll by cross-correlation: 300 ms windows at 4 kHz, at least 0.5 correlation and clearly better than any other lag, confirmed by a second window. This usually succeeds about 0.45 s after the track arrives. If nothing matches (silence), the session places the pre-roll by its timing.
  - The recorder mixes the part of the pre-roll that came before the live audio into the WAV where it belongs, and starts that carbon's Deepgram stream with it. A representative sends it to GPT-Live in one burst, followed by the live audio it held back.
  - Each placement logs `pre-roll placed`, with `prefix_ms`, `held_ms` and `correlation`, which is absent when the pre-roll was placed by timing.
- **Mixing.** libwebrtc decodes every subscribed track (Opus, with its adaptive jitter buffer and loss concealment) and delivers 10 ms frames on one playout clock, resampled to the session's rate. Each session keeps a small buffer per track (40 ms for a representative, 200 ms for the recorder and voicemail) and mixes on a 20 ms tick that follows the audio, not the wall clock.
  - When frames arrive late, on a busy host, the mix waits for them, up to 300 ms for a representative and 3 s for the recorder and voicemail. It then catches up, so a stall delays words instead of cutting holes into them. Only audio later than that is mixed as silence.
  - Latency left over after such a gap is shed by skipping quiet 10 ms blocks, never speech, while a track holds more than 100 ms (representative) or 1.2 s (recorder) of audio.
- **Voicemail player.** It joins as `ring:voicemail`, plays the greeting and a beep (Natural Voice, synthesised by Live readback and cached by voice and text; `greeting.prepare` fills the cache ahead of need, `greeting.forget` drops a deleted account's greetings, and a greeting nobody used for 30 days is deleted), records the caller's track at 48 kHz (at most 120 s), uploads the result to S3, and transcribes it with Deepgram's prerecorded API.
- **Silicon voicemail.** It synthesises the silicon's text in the silicon's voice and uploads it.

### livekit-server
A single binary pinned to v1.13.10, run by systemd with no Redis. Its configuration is in `deploy/livekit.yaml`.
- Signalling is on loopback `:7880` behind Caddy at `/rtc`.
- Audio is Opus, and publishers may add RED (redundant audio). For a publisher that sends plain Opus, LiveKit adds RED towards subscribers that accept it (`active_red_encoding`). ring-media's participants receive plain Opus over loopback.
- Media uses 7881/tcp, 7882–7892/udp and 3478/udp (TURN).
- Rooms are named by `ringid`.

### Clients
| Client | Stack | Media | Wake / ringing |
|---|---|---|---|
| iOS + iPadOS | SwiftUI (`apple/`, same project as macOS) | LiveKit Swift 2.17.0 | PushKit VoIP → CallKit; WebSocket while foreground |
| macOS | SwiftUI + AppKit | LiveKit Swift | Login item + MenuBarExtra with a persistent WebSocket; ringing panel and time-sensitive notification; APNs alert as a fallback when the app is not running |
| Android | Kotlin + Jetpack Compose (`android/`) | LiveKit Android 2.29.0 | FCM high-priority data → core-telecom + CallStyle full-screen notification |
| Windows | WinUI 3, C# (`windows/`) | `ring-ffi.dll` (Rust, LiveKit `PlatformAudio`: WASAPI + AEC3) | Tray process at login with a persistent WebSocket; IncomingCall toast plus a ringing window |
| Linux | GTK4/libadwaita, Rust (`linux/`) | `ring-core` media (LiveKit `PlatformAudio`: Pulse/ALSA + AEC3) | Autostart background process with a persistent WebSocket; portal `call.incoming` notification plus a ringing window |
| Web | Next.js 16 static export + Silicon UI (`web/`) | `livekit-client` 2.22.4 | WebSocket while open; Web Push (VAPID) service worker when closed |
| CLI (silicons) | Rust (`crates/ring-cli`) | none (the representative speaks on the server) | Ting (`ring.call.incoming`) to the silicon's local webhook |

Every client publishes one mono microphone track to the same spec: platform echo cancellation, noise suppression and gain control on, capture at 48 kHz where the platform allows it (Bluetooth HFP runs at 16 kHz), and Opus at up to 32 kbps with DTX off (so quiet word onsets are never dropped) and RED on (redundant frames against uplink loss).

When the media grant names `preroll_to`, the Apple, Android and web clients start that microphone track as they connect, not after, and stream its pre-roll (see ring-media) before and while publishing it:
- **Apple:** a `LocalAudioTrackRecorder` on the track; on iOS only once CallKit has activated the audio session.
- **Android:** a sink on the prewarmed default track, halved from 48 kHz.
- **Web:** an AudioWorklet in a 24 kHz `AudioContext`.

A device that starts muted sends none, and muting stops it. The desktop apps (`ring-core`) do not send a pre-roll yet.

Shared Rust crates:
- **`ring-proto`:** wire types.
- **`ring-core`:** client SDK, with the HTTP API, the realtime WebSocket, and LiveKit call media behind the `media` feature. Used by the CLI (API only), the Linux app and `ring-ffi`.
- **`ring-ffi`:** a C ABI over the `ring-core` media, used by Windows.

## Call lifecycle

1. **Create.** `POST /v1/calls`:
   - Resolves the callee through Accounts.
   - Creates the call (`ringing`) and an invitation (60 s), and creates the LiveKit room.
   - A carbon caller gets a media grant immediately and joins the room. Its own client plays the ringback.
   - A silicon caller's representative starts right away, so GPT-Live is connected by the time the callee answers. ring-media holds the start text until someone else's audio is in the room. If the call ends or goes to voicemail unanswered, the representative is stopped. A busy callee gets no pre-started representative.
   - When a carbon calls a callee with voicemail enabled, the server asks ring-media to prepare the callee's three greetings (`greeting.prepare`), so a caller who reaches voicemail never waits for synthesis. Each voice and text is sent once per server run, and a change of voice or greeting prepares the new ones right away.
   - If the callee is a silicon already in a reserved or active call, the invitation is `busy` straight away (see step 5).
   - If the callee blocked the caller (`blocks`), the invitation is stored `hidden`: it skips step 2 entirely, is never busy, and on timeout the call ends `missed` without voicemail. The callee's call lists, `can_view` and event audience leave it out. Everyone else sees it as an ordinary invitation: `call.updated` and `call.ended` share one rendering, and only the creator (for its `direction`) and each member with a hidden invitation get their own. The caller sees an ordinary unanswered invitation. `invite` does the same for an invitee who blocked the inviter.
   - If the callee's Ring account is deleted or still being deleted (a `pending_deletions` row, or `ring_deleted_at` with no settings row since), the invitation is ordinary but skips step 2, is never busy, and on timeout ends with no missed notice and no voicemail. This is checked in the same transaction, and again when the call would go to voicemail. `invite` does the same.
2. **Ring.** For each of the callee's devices with ringing enabled:
   - Sends a WebSocket event `call.incoming`, plus push: APNs VoIP for iOS/iPadOS, FCM for Android, APNs alert for macOS when offline, and Web Push.
   - The devices (and push kinds) the incoming push went to are recorded (`invitation_pushes`, kept a day) in a transaction that checks the invitation is still ringing and not `hidden` (a block can land after the invitation was loaded to ring), and nothing is sent otherwise; cancel pushes go only to them.
   - For a silicon callee, sends a Ting `ring.call.incoming`.
3. **Accept.** `POST /calls/{ringid}/accept`:
   - Atomically claims the invitation (and the device, for a carbon).
   - The call becomes `active`.
   - Ringing stops everywhere else: `call.ringing_stopped`, cancel pushes, and the CallKit/Telecom call ends with "answered elsewhere".
   - A carbon device gets a media grant and joins. From here on (and for a rejoin during an active call), the grant names the recorder as `preroll_to` and allows the device to send data, for its pre-roll. A ringing caller's grant does neither.
   - For each silicon participant whose representative is not already running, ring-media starts one, which speaks the start text first. A silicon caller's pre-started representative is told who answered (`roster.changed`) and then speaks its start text.
   - The recorder and STT start.
4. **During the call.**
   - Transcript entries stream to participants over the WebSocket, and to silicons through Ting in 10 s batches.
   - Delegations go to the owning silicon immediately, with the last 30 s of transcript.
   - `invite` creates more invitations that ring just like step 2.
5. **Busy, declined or missed on a direct call.** The callee is busy, declines, or the 60 s timeout passes, and voicemail is enabled:
   - The call becomes `voicemail`.
   - **Carbon caller:** ring-media plays the callee's greeting and a beep, then records the caller until they hang up (at most 120 s).
   - **Silicon caller:** gets Ting `ring.voicemail.offered` and may `POST /calls/{ringid}/voicemail {text}`.
   - The voicemail is stored, transcribed and delivered: WebSocket and alert push for carbons, Ting for silicons.
   - The call then ends.
6. **Leave or end.**
   - `leave` removes the participant from the call.
   - The call continues while 2 or more participants remain, or 1 remains with a ringing invitation. Otherwise it ends: the room is deleted, the recording is finalised and uploaded, and `call.ended` is sent.
   - A carbon whose media disconnects gets a 30 s grace period to rejoin through `POST /calls/{ringid}/media` before it is treated as having left.
7. **Handoff (carbon only).** Device B calls `POST /calls/{ringid}/media`:
   - B becomes the authoritative device.
   - A receives `call.handoff` and leaves, and the server removes A's LiveKit participant after 2 s.
   - The representatives and STT switch to B's track.
   - Nothing is written to the transcript.

## Identity and security
- Every record is keyed by the Silicon Accounts **uuid**. `c:`/`si:` ids are for display and lookup only, because they can change. The Accounts webhook keeps ids, names and photos current.
- Ring sessions are opaque tokens (`rst_…`, stored only as SHA-256 hashes). Each session is one device.
  - The Accounts refresh token is stored encrypted with AES-256-GCM (`RING_ENCRYPTION_KEY`). It is introspected without rotation when a WebSocket connects and every 10 minutes.
  - It is refreshed (under a per-session lock) only when an access token is needed to mint a Ting registration proof.
- The web app uses an HttpOnly `ring_session` cookie, and mutations require the `X-Ring-Client` header (CSRF).
- **Account deletion** (`DELETE /v1/me`, or Accounts' `account.deleted`) is first recorded in `pending_deletions` (for the webhook, in the same transaction that marks the event handled). It then runs under a per-account lock: the account leaves its live calls, its sessions are revoked, then one transaction deletes its own data and every call of its that nobody else still has (someone else has a call if they can see it, which the target of a `hidden` invitation cannot, have Ring settings, and have not deleted their own Ring account since), records the S3 keys of their audio in `pending_files`, and removes the `pending_deletions` row. A worker retries pending deletions and S3 deletes at startup and every minute (backoff 1, 2, 4 … minutes, at most an hour). While a deletion is pending, the transactions that create a session, a call, an acceptance or an invitation for that account refuse, so the deletion's scan of live calls cannot miss one. So does a settings change, during the deletion or after it, because a settings row is what marks the account as using Ring again. A request that only changes the account's other rows (a device's push tokens, for example) and was already in flight when its session was revoked can still commit after the deletion. A recording or voicemail that finishes uploading for a call or voicemail that is already gone is queued for deletion when ring-media reports it, and so is the audio of a deleted voicemail. The account's row stays as a tombstone (`on_ring = 0`, `ring_deleted_at`) with its revoked, unnamed sessions, so a repeated request is recognised and calls created before `ring_deleted_at` are never shown to it again, even after it signs in anew; the other people on a kept call still see its name and photo.
- The LiveKit API secret, Accounts app secret, Ting proofs, APNs/FCM keys and the OpenAI/Deepgram keys exist only on the server.
- Ring collects and sends no telemetry. Logs are local (`RUST_LOG`) and never contain tokens or conversation text.

## Deployment
- CloudFormation stack `silicon-ring` (us-west-1): EC2 t4g.medium (arm64, AL2023), Elastic IP `13.57.4.48`, S3 bucket `silicon-ring-assets-smdjuu0paizr`, Secrets Manager `silicon-ring/production`, SSM-only administration with no SSH.
- systemd units: `caddy`, `livekit`, `ring-server`, `ring-media`.
- Releases are tarballs in S3 (`releases/<version>-<sha>.tar.gz`). `deploy/deploy.py` installs them over SSM, with a health check and rollback.
- DNS (Namecheap) is unchanged: `ring.` and `backend.ring.` both point to the Elastic IP.
