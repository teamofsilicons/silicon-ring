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
  - A delegation with no commentary gets a "still checking" prompt at 12 s and a "will follow up" prompt at 35 s. Thinking restarts the clock, and commentary or a new GPT-Live session clears it.
- **Recorder.** A hidden participant `ring:recorder`. It subscribes to all audio, mixes it to a 16 kHz mono WAV, and uploads it to S3 when the call ends.
- **STT.** Each carbon's authoritative track is streamed to Deepgram live (`nova-3`, `language=multi`). Speakers are attributed exactly, one track per person. A silicon's speech text comes from its representative's output transcript.
- **Voicemail player.** It joins as `ring:voicemail`, plays the greeting and a beep (Natural Voice, synthesised by Live readback and cached by voice and text; `greeting.prepare` fills the cache ahead of need), records the caller's track (at most 120 s), uploads the result to S3, and transcribes it with Deepgram's prerecorded API.
- **Silicon voicemail.** It synthesises the silicon's text in the silicon's voice and uploads it.

### livekit-server
A single binary pinned to v1.13.10, run by systemd with no Redis. Its configuration is in `deploy/livekit.yaml`.
- Signalling is on loopback `:7880` behind Caddy at `/rtc`.
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
2. **Ring.** For each of the callee's devices with ringing enabled:
   - Sends a WebSocket event `call.incoming`, plus push: APNs VoIP for iOS/iPadOS, FCM for Android, APNs alert for macOS when offline, and Web Push.
   - For a silicon callee, sends a Ting `ring.call.incoming`.
3. **Accept.** `POST /calls/{ringid}/accept`:
   - Atomically claims the invitation (and the device, for a carbon).
   - The call becomes `active`.
   - Ringing stops everywhere else: `call.ringing_stopped`, cancel pushes, and the CallKit/Telecom call ends with "answered elsewhere".
   - A carbon device gets a media grant and joins.
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
- The LiveKit API secret, Accounts app secret, Ting proofs, APNs/FCM keys and the OpenAI/Deepgram keys exist only on the server.
- Ring collects and sends no telemetry. Logs are local (`RUST_LOG`) and never contain tokens or conversation text.

## Deployment
- CloudFormation stack `silicon-ring` (us-west-1): EC2 t4g.medium (arm64, AL2023), Elastic IP `13.57.4.48`, S3 bucket `silicon-ring-assets-smdjuu0paizr`, Secrets Manager `silicon-ring/production`, SSM-only administration with no SSH.
- systemd units: `caddy`, `livekit`, `ring-server`, `ring-media`.
- Releases are tarballs in S3 (`releases/<version>-<sha>.tar.gz`). `deploy/deploy.py` installs them over SSM, with a health check and rollback.
- DNS (Namecheap) is unchanged: `ring.` and `backend.ring.` both point to the Elastic IP.
