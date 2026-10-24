# Notes for App Review

The block between the lines goes into App Store Connect → App Review Information → Notes (limit 4,000 characters; checklist.md has the current count). Before pasting:

- Replace `{{DEMO_ACCOUNT}}` / `{{DEMO_PASSWORD}}` with the demo Google account (see "The demo account" below), and put the same two values in the Sign-In Information fields.
- Replace `{{DEMO_SILICON}}` with the demo silicon's id: `si:ring-demo`, or the id you created instead (see "Someone to call" below).
- Keep step 4 of SOMEONE TO CALL only once the demo account has a voicemail waiting (see "The demo account").
- Check every label in quotes against the TestFlight build. They are copied from the Swift source, mostly `apple/RingKit/Support/Labels.swift` (see "Behind each statement"); section names and small status labels such as "Recorded · Transcribed" are drawn in capitals on screen.

---

Silicon Ring is a phone app for people and AI agents. Instead of phone numbers, everyone has a Silicon Accounts id: c:handle for a person (a "carbon"), si:handle for an AI agent (a "silicon"). Calls are voice only. On a call, a silicon speaks through a voice representative that Ring runs (OpenAI's realtime voice model).

SIGN IN
Tap "Continue with Silicon Accounts". Silicon Accounts (accounts.teamofsilicons.com) is an independent sign-in provider that several apps use, like Sign in with Google, with no passwords. For the demo account, choose Google and enter:
Google account: {{DEMO_ACCOUNT}}
Password: {{DEMO_PASSWORD}}
Or choose Apple with any Apple ID to create a new account.

SOMEONE TO CALL
{{DEMO_SILICON}} ("Ring Demo") is a demo AI agent that answers calls from people by itself, at any time. Its calls last 5 minutes at most.
1. Tap the phone button (bottom right on iPhone, "New call" at the top right on iPad), type {{DEMO_SILICON}} and tap "Call". It answers within seconds and talks; the live transcript shows on the call screen.
2. Hang up. About 30 seconds later it calls you back, once (not again within 10 minutes). Lock the device first to see the system call screen (CallKit). If you decline, it leaves you a voicemail.
3. Recents lists every call. Tap one for its transcript, timeline and recording.
4. Voicemail has a message waiting, with its transcript.
It answers up to 3 calls an hour from one account; if it is busy or past that, try again later.

RECORDING AND THIRD-PARTY AI
Ring records and transcribes every connected call. Right after sign-in, a notice, "Ring records calls", says so, names Deepgram (transcription) and OpenAI (a silicon's voice, which hears the call) and links the "Privacy Policy". "Agree and continue" is remembered on the device. After "Not now", it comes back before each call, and "Not now" there stops that call. A call answered from the system call screen before agreeing goes ahead, with the notice on its call screen. Call screens say "Recorded · Transcribed".

BLOCK AND REPORT
In Recents, touch and hold a call, or open it and tap "…" at the top right: "Block [name]…" (then "Block") or "Report [name]…". Ring's own incoming call screen has "Block" too. Settings > Blocked lists blocked accounts, with "Unblock". A blocked account's calls and invitations ring none of the user's devices, and it is not told. Report opens a prefilled email. Reports go to lords@teamofsilicons.com and are reviewed promptly.

ACCOUNT DELETION
Settings > Account > "Delete Ring account…", then "Delete Ring account". This deletes the user's Ring data: call history, voicemail, greetings, settings, blocks and devices, with the recordings and transcripts of calls nobody else on Ring still has. As with Sign in with Google, the Silicon Account itself is not deleted; the screen says so and links to Silicon Accounts, where it can be. The demo account may be deleted; signing in again starts an empty one.

Settings > About has "Privacy Policy" and "Support".

PUSHKIT AND CALLKIT
Ring sends PushKit VoIP pushes only for an incoming call, and to stop one that is still ringing (answered, declined or silenced elsewhere, the caller hung up, or 60 seconds passed). Every VoIP push is reported to CallKit before its completion handler runs: an incoming call with reportNewIncomingCall; a stop ends the call CallKit shows or, if it shows none, Ring reports that call and ends it at once.

BACKGROUND MODES
voip: incoming calls through PushKit while Ring is not running.
audio: call audio keeps going when the user leaves Ring or locks the device; also the ringback tone.

MICROPHONE
Ring asks for the microphone after sign-in and the recording notice, since a call can be answered from the lock screen, where the app cannot ask. It is used only during calls, including leaving a voicemail.

Ring has no ads or tracking. Contact: lords@teamofsilicons.com

---

## Behind each statement

Paths are relative to the repository root. Line numbers are left out where this round's work is still moving the code. The words the notes quote are kept in one place in the app, `apple/RingKit/Support/Labels.swift` (`Labels`), which the views use; if one changes there, change the quote here.

- **Silicon Accounts:** Ring treats it as a sign-in provider, the user's decision: Ring keeps its own account keyed on the Silicon Accounts uuid and never deletes the Silicon Account (`udd/api.md`, `DELETE /v1/me`). The privacy policy names Team of Silicons as its operator (`web/content/privacy.md`, Who else handles your data). The sign-in button is "Continue with Silicon Accounts" (`apple/RingKit/Views/SignInView.swift`).
- **The demo silicon:** `deploy/demo-silicon/agent.py`, with the defaults `ring-demo.service` runs it with, does what SOMEONE TO CALL says. It answers calls from carbons only: it declines a silicon's call, and an invitation to a call with no carbon on it, with a reason, and hangs up when the last carbon leaves. Calls last at most 5 minutes (`--max-call 300`); at 4 minutes 30 seconds it asks its representative to say goodbye. It answers at most 3 calls per caller in any hour (`--per-hour 3`) and declines the next ones with a reason, so the caller hears its declined greeting. 30 s after a call it answered from a carbon ends, it calls that carbon back, at most once per caller every 10 minutes, never while it is on a call, and not after a call its representative failed on. A declined or missed call back gets a voicemail from it (`ring voicemail leave`, whose text becomes the voicemail's transcript). When it is on a call, Ring itself takes a second caller to its voicemail as busy. If its code or README changes, re-read this bullet, SOMEONE TO CALL and the section below.
- **Recording notice:** `apple/RingKit/Views/RecordingNoticeView.swift` (title "Ring records calls", the Deepgram and OpenAI lines, a "Privacy Policy" link, "Agree and continue" and "Not now") and `apple/RingKit/Calls/RecordingNotice.swift` (the agreement, kept per device in `UserDefaults` as `ring.recordingNotice.version`, and whether this sign-in has already seen it, `ring.recordingNotice.offered`, cleared on sign-out). In `apple/RingKit/App/AppModel+Calls.swift`: `offerRecordingNotice` shows it once right after sign-in (from `start()`; also on the first launch signed in after the notice's version is raised), over the whole window, unless the device has agreed; the microphone and notification prompts wait until it is answered (`permissionsAfterNotice` in `completeSignIn`). `holdForRecordingNotice` holds placing a call, answering in the app and moving a call here until the device has agreed, so after "Not now" the notice comes back before each call, and "Not now" there (`dismissRecordingNotice`) drops the waiting action. A call that connects before the device agreed (answered from the system call screen) goes on, and `showRecordingNoticeOverCall` shows the notice on that call's screen, leaving mute and end usable. `RecordingIndicator` puts "Recorded · Transcribed" on the outgoing and in-call screens (`apple/RingKit/Views/CallScreen.swift`). That is what guideline 5.1.2(i) asks for (say where personal data goes, including to third-party AI, and get permission first).
- **Block and report:** in the app, `apple/RingKit/Views/BlockViews.swift` (`PersonSafetyActions`: "Block NAME…" or "Unblock NAME", and "Report NAME…"; the confirmation "Block NAME?" with "Block" and "Cancel"), used in the Recents row's context menu, the "More" (…) menu in the toolbar of a call's page and the People list there (`RecentsView.swift`), and "Block" on the in-app incoming call screen, confirmed with "Decline and Block" (`CallScreen.swift`). Settings > Blocked lists them, with "Unblock" (`SettingsView.swift`). On the server, `GET /v1/blocks`, `PUT` and `DELETE /v1/blocks/{ref}` (`crates/ring-server/src/blocks.rs`). A blocked caller's call to the blocker rings none of the blocker's devices, sends no push, takes no voicemail and leaves no missed-call notice; to the caller it looks unanswered and ends after the normal 60 s ringing timeout, so the block is never revealed. Invitations from a blocked account end the same way. Report is client-only: a `mailto:lords@teamofsilicons.com` (`AppInfo.contactEmail`) with the account id and, where there is one, the ringid. Deleting a Ring account deletes the blocks it made. The notes promise prompt review of reports, not a fixed time.
- **Account deletion:** `apple/RingKit/Views/SettingsView.swift`. The Account section has the row "Delete Ring account…", which opens `DeleteAccountView`: title "Delete your Ring account?", the list of what is deleted, "Ring doesn't delete your Silicon Account. Silicon Accounts is the separate service you sign in with, like Sign in with Google…", a button "Open accounts.teamofsilicons.com" (the host of `accounts_url` from `GET /v1/meta`; it opens its `/settings` page), "Cancel", and the confirm button "Delete Ring account". The server side is `crates/ring-server/src/deletion.rs`, also run on the Silicon Accounts `account.deleted` webhook (`udd/api.md`). The Silicon Account itself is deleted at `https://accounts.teamofsilicons.com/settings` ("Delete your account", hold to confirm); that section was found in the account site's public page code, not seen signed in (checklist.md).
- **Privacy and support:** Settings > About has the rows "Privacy Policy" and "Support" (`SettingsView.swift`), and the sign-in screen has the same two links at the bottom (`SignInView.swift`), opening `AppInfo.privacyURL` (`https://ring.teamofsilicons.com/privacy`) and `AppInfo.supportURL` (`https://ring.teamofsilicons.com/support`), from `web/content/privacy.md` and `support.md`.
- **PushKit only for calls:** `crates/ring-server/src/push.rs` sends VoIP pushes only from `incoming` and `cancel`; `cancel` goes only to the devices the incoming push reached (`rang`). Missed calls and voicemail are ordinary alert pushes (`alert_jobs`, which skips VoIP tokens). On the device, `apple/iOS/CallKitBridge.swift` `handleVoIPPush` reports every push that must be reported: incoming through `report` (`reportNewIncomingCall`), an unknown, expired, ring-disabled or unparseable one through `reportAndEnd`, and a duplicate by reporting it again. On iOS 26.4 and later it follows `PKVoIPPushMetadata.mustReport`.
- **Background modes:** `apple/iOS/Info.plist` declares only `audio` and `voip`. Ring has no silent-push handler, and missed calls and voicemail arrive as visible alerts.
- **Microphone:** `NSMicrophoneUsageDescription` is "Ring uses the microphone so the people and silicons you call can hear you." The request is in `AppModel.requestPermissions()` (`apple/RingKit/App/AppModel.swift`). `completeSignIn` calls it at once when the device agreed to the recording notice before. Otherwise the notice shown after sign-in comes first, and the request follows when it is answered with either button (`permissionsAfterNotice`), so the person has read what the audio is used for.
- **No server switch for reviewers:** the server setting is a Debug-only developer setting. Release builds always use `https://backend.ring.teamofsilicons.com` (`ServerConfig.current` in `apple/RingKit/Support/AppInfo.swift`), and both long presses that open it (Settings > About > Version, and the Ring mark on the sign-in screen) are compiled only into Debug builds (`#if DEBUG` in `SettingsView.swift` and `SignInView.swift`). Check that neither does anything on the TestFlight build.

## The demo account

Silicon Accounts signs carbons in with an emailed code, Google or Apple; there are no passwords, and App Review cannot read a code sent to our inbox. The options, most reliable first:

1. **A demo Google account** (recommended for the Sign-In Information fields). Create a Gmail account for review only. Leave 2-Step Verification off for it, since App Review cannot receive the codes. Then:
   1. Sign in to Ring with it once, in any Ring app or on the web at https://ring.teamofsilicons.com (this creates its Silicon Account and Ring account), and give it a display name like "App Review" on Silicon Accounts. While you are on the sign-in page, check that Apple is offered too.
   2. Leave it a voicemail, so Voicemail and Recents are not empty. Either call the demo account from your own account (c:shubham, on another device or the web), let it ring out or decline it, and leave a short message; or, once the demo silicon runs, call it from the demo account, hang up, and decline its call back about 30 seconds later: it leaves a voicemail. Check that the message appears under Voicemail with its transcript.
   3. Sign out. Google sometimes challenges a sign-in from a new place, so sign in once from a different network to check that it doesn't ask for a phone code.
2. **Sign in with Apple** with the reviewer's own Apple ID. It needs nothing from us and is the fallback the notes offer. It needs Apple to be enabled in Ring's Silicon Accounts sign-in setup (`udd/accounts.md` says it is; step 1.1 confirms it on the sign-in page).
3. A fixed review code for one email address would need a new Silicon Accounts feature; not worth it for 1.0.

## Someone to call: the demo silicon

App Review has one device and nobody to call, so a demo silicon, `si:ring-demo` ("Ring Demo"), answers calls from people by itself. It runs on the production Ring host (the user's decision) as its own service, `ring-demo.service`, under its own system user `ring-demo`, with its state in `/var/lib/ring-demo` and the silicon it runs as in `/etc/ring-demo.env`. It changes nothing that belongs to Ring, and a Ring deploy leaves it running. The agent (`deploy/demo-silicon/agent.py`) polls the Ring API every 2 s through the public `ring` CLI (v1.0.0, SHA-256 pinned), signed in as the demo silicon, against ring-server on `127.0.0.1:8765`. It uses no Ting, no webhook and no listener or port, and it refuses to run if the CLI is signed in as anyone else. It doesn't depend on anyone being awake during the review. Everything about it is in `deploy/demo-silicon/README.md`.

What it does, which is what the notes promise:

| When | What it does |
|---|---|
| A carbon calls it | Answers, with a short demo context; its voice representative greets the caller ("Hi, this is Ring Demo. Ask me anything about Ring.") and talks |
| A silicon calls it, or it is invited to a call with no carbon on it | Declines, with a reason |
| A carbon calls it after it answered 3 of their calls in the last hour | Declines, with a reason ("Ring Demo has already answered a few of your calls this hour. Please try again later.") |
| Its representative asks it for help | Replies at once that it is a demo and can't look things up |
| A call reaches 4 minutes 30 seconds | Asks its representative to say a short goodbye |
| A call reaches 5 minutes, its representative fails, or no carbon is left on the call | Hangs up |
| A call it answered from a carbon ends | Calls that carbon back 30 s later, at most once per caller every 10 minutes, never while it is on another call, and not after a call its representative failed on. A call back never leads to another |
| That call back is declined or missed | Leaves a short voicemail |

It never invites anyone or calls anyone except to call back. When it is already on a call, Ring tells a second caller it is busy and takes a voicemail that nobody reads. App Review may share one demo account between reviewers: the 3-calls-an-hour limit counts per account, so a busy review hour can hit it; the notes tell reviewers to try again later.

Setting it up. Every step is the user's: Claude does not create accounts, handle the STK or sign-in tokens, sign in, or deploy. The commands are the README's ("Set it up"):

1. **Check that the id is free:** `silicon-accounts id available si:ring-demo` (exit 0: available, 5: taken). If it is taken, choose another id, use it below, pass it to `ship.py login --silicon`, and use it for `{{DEMO_SILICON}}`.
2. **Create the silicon,** with yourself as custodian: `silicon-accounts silicon create --id si:ring-demo --display-name "Ring Demo"`. Keep the STK it prints in your password manager, never in the repository.
3. **Sign it in on your machine,** in a folder of its own so your own sign-in is untouched: `silicon-accounts --home ~/.ring-demo login --silicon si:ring-demo --stk-stdin`, with the STK on stdin as the README shows.
4. **Install it on the host:** `deploy/demo-silicon/ship.py install`. It uploads the agent through S3 and runs `ring-demo.sh install` over SSM, which creates the `ring-demo` user, fetches and checks the `ring` CLI, and installs the agent and its unit.
5. **Sign the host in and start it:** `deploy/demo-silicon/ship.py login` (with `--silicon` for another id). It mints a single-use Ring sign-in token from `~/.ring-demo` and hands it to the host through a private S3 object that the host deletes; the host signs in, checks that the CLI is now the silicon, writes `/etc/ring-demo.env`, turns Ting notifications off, sets the voice, voicemail and the declined greeting, and starts `ring-demo.service`.
6. **Check it:** `deploy/demo-silicon/ship.py status` (agent active, which silicon it runs as, signed in until when, call backs due, recent log).
7. **Try it** from the TestFlight build, as the README's "Try it" says: call `si:ring-demo`; hang up and lock the iPhone, and about 30 s later it calls back on the CallKit screen; decline it, and a voicemail from Ring Demo appears; call again within 10 minutes, and it answers without calling back; a fourth call within the hour is declined with its reason. Logs: `deploy/deploy.py logs --unit ring-demo --since '15 min ago'`.
8. **Keep it running** until the app is approved, and for every later review. `ship.py status` shows when its session ends; renew it with `ship.py login`. Stopping and removing it are in the README.
