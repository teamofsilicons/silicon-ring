# Ring web client

SolidJS + TypeScript + Vite. Run from the repository root:

```sh
npm ci --prefix web
npm run dev --prefix web    # http://localhost:1420
npm test --prefix web
npm run build --prefix web
```

The production web sign-in screen offers **Continue as Carbon**, which opens IAM with `app_id=ring`, `identity_kind=carbon`, and a callback URL. IAM returns its short-lived token to that URL; Ring validates the one-use, ten-minute state stored in the initiating tab, immediately removes the token/state from the address bar, and exchanges the token through the backend. The callback is bound to the original server, realm, and browser URL. IAM chooses the organization, which the backend verifies. A token can also be pasted manually; connection settings accept an endpoint, organization, realm, and local test secret.

Production defaults to `wss://backend.ring.teamofsilicons.com/ws` (overridable at build time with `VITE_RING_SERVER`); local development defaults to `ws://127.0.0.1:8765/ws`. The backend advertises the IAM browser URL through `app.info.iam_login_url` (configured with `RING_IAM_LOGIN_URL`). The opaque Ring device/session and connection settings persist in local storage across reloads, tabs, and browser restarts, with migration from older tab storage. IAM access and refresh tokens remain on the backend; a validated Carbon callback token and its exact exchange request ID are retained only in tab storage for at most two minutes, so a reload can retry an uncertain exchange. That temporary record is removed on success, sign-out, rejected authentication, expiry, or a manual token change. Resumed session metadata is saved, transient IAM/network failures preserve sign-in, and logout or a rejected session removes the saved session. Sign out on shared devices. Test realm requires an explicitly configured backend test secret.

The UI supports incoming/calling/live states, native or browser ringing, accept, swipe decline/reasons, silence, call groups and invites, mute, active-device transfer, history with transcript and recording, missed calls, private voicemail playback/recording, profile/photo uploads, and actor preferences. It reads server state and protocol events; it contains no demo call data. Transcription rows coalesce server revisions by segment ID.

Audio uses an AudioWorklet to produce 20 ms PCM s16le mono frames at 24 kHz from common hardware sample rates. RMS VAD supplies advisory `media.speech` markers; the backend decides interrupt policy. Playback is queued with bounded latency. Microphone access needs HTTPS or localhost and user permission. The native mobile wrapper delegates audio to Swift/Kotlin so an active call does not depend on webview timers. Voicemail audio remains private to the voicemail attachment and is sent only after the greeting/beep.

Browser verification covered two-user sign-in, call/incoming/accept/end lifecycle, synchronized history, profile saves, and a 390 px viewport. Live microphone permission, audio hardware routing, and physical mobile push delivery require device testing; the backend protocol and PCM framing tests cover transport separately.

Telemetry is sent through authenticated `telemetry.record` WebSocket requests after effective preferences load. Only event source/step/progress, random trace IDs, and optional durations are emitted; actor IDs, call IDs, page names, URLs, transcript text, and credentials are excluded. The backend enforces actor/organization opt-out and production/test isolation. Remote endpoints require WSS; plaintext WS is accepted only on loopback.
