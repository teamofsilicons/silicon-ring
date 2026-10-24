# App Privacy answers (App Store Connect → App Privacy)

These are the answers for Silicon Ring 1.0 on iPhone and iPad, worked out from the code at commit `9443a87` plus this round's work: account deletion, Block and Report, the recording notice, the Deepgram opt-out and the backup expiry. Paths are relative to the repository root, and line numbers are at `9443a87` (this round moves some of them). If the code changes what it stores or who it sends it to, update this file, the app's privacy manifest (`apple/Resources/PrivacyInfo.xcprivacy`) and the label together.

This round changes no answer: no new data type, no new third party, and nothing used for tracking.

## Short version

| Question | Answer |
|---|---|
| Do you or your third-party partners collect data from this app? | **Yes** |
| Is any data used to track the user? | **No.** There is no advertising, no ad or analytics SDK, no IDFA, no App Tracking Transparency prompt, and no data is sold or given to data brokers. The only third-party SDK in the app is LiveKit (open source, talking only to Ring's own self-hosted LiveKit server); its privacy manifest declares no collected data and no tracking. |
| Purposes | **App Functionality** for every data type. Nothing is used for analytics, advertising, marketing or personalisation. |
| Linked to the user? | **Yes** for every data type: Ring keys every record on the user's Silicon Accounts uuid (`udd/architecture.md`, Identity and security). |

## Data types to tick

Answer each one as: collected, linked to the user, not used for tracking, purpose App Functionality.

| Apple data type | What Ring keeps | Where in the code |
|---|---|---|
| **Contact Info → Name** | The display name from Silicon Accounts, shown to the people and silicons the user calls. | `crates/ring-server/src/db.rs:109` (`accounts.display_name`), kept current by the Accounts webhook (`crates/ring-server/src/hooks.rs`) |
| **Contact Info → Email Address** | Ring's own server never receives an email address: the app asks Silicon Accounts only for `scope=profile` (`apple/RingKit/Auth/SignIn.swift:25`). It is declared because a new user types their email into Silicon Accounts during sign-in inside the app (the hosted page in an `ASWebAuthenticationSession`), and Silicon Accounts is Team of Silicons' own service, so this is the developer collecting it. | Silicon Accounts sign-in methods: email or phone code, Google, Apple |
| **Contact Info → Phone Number** | Ring's own server never receives a phone number. It is declared for the same reason as Email Address: Silicon Accounts also signs people in with a code sent to their phone, and a new user can type their number into it during sign-in inside the app. | Silicon Accounts sign-in methods: email or phone code, Google, Apple (accounts.teamofsilicons.com/llms-full.txt) |
| **User Content → Audio Data** | The audio of every connected call, recorded at 48 kHz and stored in S3; voicemail the user records for someone. While a call is live, each carbon's audio also goes to Deepgram for the transcript and, when a silicon is on the call, to OpenAI for the silicon's representative (see Third parties). | Recorder: `crates/ring-media/src/recorder.rs`, upload `crates/ring-media/src/services.rs:50`; voicemail: `crates/ring-media/src/voicemail.rs`; rows: `db.rs:158` (`calls.recording_key`), `db.rs:265` (`voicemails.audio_key`) |
| **User Content → Other User Content** | Call transcripts (each person's words, attributed to them), voicemail transcripts, voicemail greetings the user writes, decline reasons, and invitations. | `db.rs:218` (`transcript_entries`), `db.rs:265` (`voicemails.transcript`), `db.rs:285` (`settings.greeting_*`), `db.rs:195` (`invitations.reason`) |
| **Identifiers → User ID** | The Silicon Accounts uuid and the `c:`/`si:` id. | `db.rs:109` (`accounts.uuid`, `accounts.id`) |
| **Identifiers → Device ID** | A Ring device id per sign-in, plus the APNs alert token and the PushKit VoIP token, used to ring this device. | `db.rs:123` (`sessions.device_id`), `db.rs:142` (`push_tokens`); sent by `apple/RingKit/App/AppModel.swift:258` (`registerDevice`); tokens from `apple/iOS/CallKitBridge.swift` (`didUpdate pushCredentials`) and `apple/iOS/RingApp.swift` (`didRegisterForRemoteNotificationsWithDeviceToken`) |
| **Usage Data → Other Usage Data** | Call history: who called whom, when, how long it rang and lasted, the outcome; and each device's online state and last-seen time for Settings → Devices. | `db.rs:158` (`calls`), `db.rs:177` (`participants`), `db.rs:195` (`invitations`), `db.rs:123` (`sessions.last_seen_at`) |
| **Diagnostics → Performance Data** | Call audio quality per track (codec, bitrate, packets received and lost, jitter, concealment), written to the server's own log with the call id and the LiveKit identity when a track ends. It never contains what was said. | `crates/ring-media/src/audio.rs:611` (`log_reception`); log cap `deploy/stack.yaml:184` (journald, 512 MB) |
| **Other Data → Other Data Types** | The device's name (iOS reports a generic "iPhone"/"iPad", which Ring turns into "First-name's iPhone"; the user can rename it), platform and app version, shown in Settings → Devices. The accounts the user has blocked. | `apple/RingKit/App/AppModel.swift:163` (sign-in `label`), `:245` (`nameDeviceIfGeneric`), `:263` (`platform`, `appVersion`); `db.rs:123` (`sessions.name`, `platform`, `app_version`); blocks: `GET/PUT/DELETE /v1/blocks` (this round, `udd/api.md`) |

## Data types to leave unticked

| Apple data type | Why not |
|---|---|
| Physical Address, Other User Contact Info | Neither Ring nor its Silicon Accounts sign-in asks for an address or other contact details; Ring stores none. |
| Photos or Videos | The app never uploads a photo. Profile photos are changed on the Silicon Accounts website, and Ring stores only their URL (`accounts.pfp_url`). |
| Emails or Text Messages, Gameplay Content, Customer Support | Ring has no messaging, games or in-app support. Decline reasons are listed under Other User Content. Report opens the user's own mail app with an email to lords@teamofsilicons.com, prefilled with the reported account's id and the call's id; the app itself sends nothing, and the user decides whether to send it. |
| Contacts | Ring never reads the address book. "Recent people" in the dialer comes from the user's own call history. |
| Location (precise or coarse) | No location APIs. The client IP is used only in memory to rate-limit sign-in (`crates/ring-server/src/extract.rs:194`, `crates/ring-server/src/sessions.rs:237`) and is never stored or turned into a location; Caddy keeps no access log (`deploy/Caddyfile` has no `log` directive). |
| Health, Fitness, Financial Info, Sensitive Info, Purchases | None of this exists in Ring. Speakers are attributed by their audio track, not by voice biometrics. |
| Browsing History, Search History | The dialer looks up the id the user types; the lookup is not stored against the user. |
| Crash Data, Other Diagnostic Data | No crash reporter or analytics SDK. Ring sends no telemetry (`README.md`, `udd/architecture.md`). |
| Product Interaction, Advertising Data | No analytics, no ads. |

On-device only, so not "collected": the Ring session token in the Keychain (`apple/RingKit/Auth/SessionStore.swift:44`, `AfterFirstUnlockThisDeviceOnly`) and a few preferences in `UserDefaults` (appearance, last tab, the Debug-only server override, whether this device agreed to the recording notice, and whether this sign-in has already seen it). If the recording-notice agreement is ever sent to the server, add it to Other Data Types.

## Third parties that receive data

| Service | Operated by | What it receives | Where |
|---|---|---|---|
| Silicon Accounts | Team of Silicons (first party) | Sign-in (email, or the Google or Apple identity), profile lookups by uuid or id | `crates/ring-server/src/accounts.rs`, `udd/accounts.md` |
| Ting | Team of Silicons (first party) | Call events, transcript batches and delegations, delivered only to the silicons on the call | `crates/ring-server/src/ting.rs`, `udd/api.md` (Ting) |
| OpenAI (GPT-Live, `gpt-live-1`) | OpenAI | When a silicon is on a call: the mixed live audio of the other participants, for the silicon's representative. Greeting text and silicons' voicemail text, to speak it in a Natural Voice. | `crates/ring-media/src/live.rs:18` (`wss://api.openai.com/v1/live/sessions`), `live.rs:275` (`synthesize`) |
| Deepgram (`nova-3`) | Deepgram | Each carbon's live call audio on every call (also carbon-only calls), and recorded voicemail audio, to transcribe it. Both requests set `mip_opt_out=true`, so Deepgram does not keep the audio for its Model Improvement Program | `crates/ring-media/src/stt.rs` (`LIVE_URL` for live calls, `transcribe` for voicemail) |
| AWS | Amazon Web Services | Hosting in us-west-1 (EC2) and S3 storage of recordings, voicemail audio and daily database backups, encrypted at rest (SSE-S3, AES-256), HTTPS only | `deploy/stack.yaml` (bucket and policy), `crates/ring-media/src/services.rs:50`, `deploy/backup.py` |
| Apple Push Notification service | Apple | Push payloads: the caller's id and display name and who else is on the call (incoming calls), "Missed call" with the caller, and "Voicemail from …" with the first 180 characters of its transcript | `crates/ring-server/src/push.rs:554` (`incoming`), `:698` (`missed`), `:715` (`voicemail`) |

Firebase Cloud Messaging and Web Push are used by the Android and web apps only, never by the iOS app. LiveKit is self-hosted on Ring's own host (`deploy/livekit.yaml.tmpl`), so it is not a third party.

Apple treats processors that work on the developer's behalf as part of the developer's own collection: they do not change the answers above, and none of them is tracking. They do belong in the privacy policy, by name.

## Retention, for the privacy policy

| Data | Kept until |
|---|---|
| Ring profile, settings, greetings, blocks | The account is deleted in Ring (Settings > Account > "Delete Ring account…") or on Silicon Accounts (`account.deleted` webhook); a block also ends when the user unblocks. The privacy policy (`web/content/privacy.md`) says Ring then keeps only the uuid, id, name and photo address, so shared calls still show who was on them |
| Calls, transcripts, recordings | The account is deleted. A call shared with others stays for them until none of them has a Ring account (account deletion spec). There is no automatic expiry. |
| Voicemail | The recipient deletes it (the S3 audio is deleted too, `crates/ring-server/src/voicemail.rs:393`) or the account is deleted |
| Push tokens | Sign-out, device revoke, account deletion, or APNs reporting the token invalid (`push.rs:457`) |
| Ting deliveries to silicons | 14 days after delivery (`crates/ring-server/src/main.rs:183`) |
| Server logs | Rolling, capped at 512 MB on the host (`deploy/stack.yaml:184`) |
| Database backups | **35 days in S3.** A daily SQLite backup goes to `s3://…/backups/` (`deploy/systemd/ring-backup.timer`, `deploy/backup.py`), and this round adds an S3 lifecycle rule on `backups/` to `deploy/stack.yaml` that expires each one after 35 days. Until the user applies that stack change, nothing expires them. The host also keeps its last 7 daily backups on its encrypted disk (`backup.py --keep-local`, default 7). So a deleted account is gone from every backup at most 35 days after it is deleted, as the privacy policy says. |

## Things that affect these answers and the privacy policy

1. **Deepgram model improvement: resolved.** Both Deepgram requests now set `mip_opt_out=true` (`crates/ring-media/src/stt.rs`), so call and voicemail audio is not used to improve Deepgram's models. It takes effect when the user deploys ring-media. It never changed the label (it is not tracking); the privacy policy says it.
2. **OpenAI API data.** OpenAI's API terms at the time of writing do not train on API data by default and keep it up to 30 days for abuse monitoring; check that this covers GPT-Live and say it in the privacy policy.
3. **Guideline 5.1.2(i): resolved in this round.** It asks apps to say clearly where personal data goes to third parties, including third-party AI, and to get explicit permission first. Right after sign-in, Ring shows a notice ("Ring records calls") saying that calls are recorded and transcribed, that Deepgram transcribes them and that OpenAI produces a silicon's voice and hears the call, with a link to the privacy policy. "Agree and continue" is remembered on the device; until then the notice comes back before placing, answering or moving a call there, and "Not now" stops that call (`apple/RingKit/Views/RecordingNoticeView.swift`, `apple/RingKit/Calls/RecordingNotice.swift`).
4. **The privacy manifest** is in the app as of this round: `apple/Resources/PrivacyInfo.xcprivacy`, listed in the `RingApp` target template in `apple/project.yml`, so xcodegen bundles it in both the iOS and the Mac app. It declares the nine data types above (all linked, none for tracking, App Functionality only) and Ring's one required-reason API (`UserDefaults`, reason `CA92.1`). Keep it and this file in step.
5. **The privacy policy** (`web/content/privacy.md`) is updated in this round to match: backups deleted from S3 after 35 days, the Deepgram opt-out, and blocks (kept until unblocked or the account is deleted). Re-read it against this file before the web deploy.
