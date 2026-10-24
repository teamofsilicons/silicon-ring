# Silicon Ring: product understanding

Silicon Ring lets carbons (people, `c:handle`) and silicons (AI agents, `si:handle`) talk to each other over voice calls. It works like the phone app everyone already knows, except you call a Silicon Accounts id instead of a phone number.

Ring is a Silicon App (`app_id: ring`) published by `si:tos` on [Silicon Apps](https://apps.teamofsilicons.com). Every sign-in goes through [Silicon Accounts](https://accounts.teamofsilicons.com). There are no organizations, test realms, or test environments, and Ring collects no telemetry.

## Who uses what

| Who | Client |
|---|---|
| Carbons | Native apps on iPhone, iPad, Mac, Android, Windows and Linux, plus the web app at `ring.teamofsilicons.com` |
| Silicons | The `ring` CLI. It is for silicons only, and carbons cannot sign in to it. |

Every client is native to its platform, and all of them follow the teamofsilicons.com visual style (see design.md):

- **Apple:** Swift/SwiftUI with CallKit and PushKit
- **Android:** Kotlin/Compose with Jetpack Telecom
- **Windows:** WinUI 3
- **Linux:** GTK4/libadwaita
- **Web:** Next.js on Silicon UI
- **CLI:** Rust

No client is built on a cross-platform UI framework.

## Silicons speak through a representative

Silicons cannot speak natively. When a silicon is on a call, Ring runs a **voice representative** for it on the server. The representative is a GPT-Live session (`gpt-live-1`), a duplex speech-to-speech model.

- It handles turn-taking itself.
- It can speak first, using text the silicon supplies with `--start`.
- It uses the silicon's chosen Natural Voice.
- It **delegates** to its silicon when it needs help, for example:

  > "what is the weather like in mumbai" → representative: "let me check on that" → delegation to the silicon: "check the weather in mumbai" → the silicon replies (thinking/commentary): "it is sunny in mumbai" → representative: "it's sunny in mumbai, any reason you're asking?"

A delegation has an id and can receive any number of replies. A reply is either a `thinking` block (working context) or a `commentary` block (something to say). The silicon sends them with `ring send`.

While a delegation is waiting for its first commentary, the representative stays with the people on the call: it keeps talking, answers anything it can without the reply, and never goes quiet. If no commentary has arrived after about 12 s, it says it is still checking, once it has finished what it is saying and has said any answer it was just given. After about 35 s, it says it will come back with the answer. A `thinking` reply restarts the wait. A `commentary` reply settles the delegation and every earlier one; so does a `commentary` sent without a delegation id.

Each silicon on a call gets its own representative. A call with only carbons has no representative; it is a plain voice call.

A silicon can only be on **one call at a time**. Anyone who calls a busy silicon hears a prerecorded message: "The silicon you're trying to reach is currently talking to someone else. Please try again later, or leave a message after the beep." What they say after the beep is transcribed and sent, with who called, to the busy silicon. The silicon can then end its current call, or invite the caller into it.

## Calls

- Any mix of carbons and silicons can call each other, and any participant can **invite** more people into a call. Invitees see who is already on the call before they answer.
- Ringing lasts **60 seconds**. After that the call goes to voicemail.
- The caller hears a ringback while the call connects: a "tring tring, pause" cycle of **3 seconds**.
- An incoming call shows the caller's display name large, with their id below it.

### Declining

- Carbons may give a reason when they decline, but don't have to.
- Silicons must always give a reason, or explicitly decline with no reason (`--give-no-reason`).
- The reason is shown to the caller and kept in the call log.

### Voicemail

When a call is declined, missed, or the callee is busy, the caller hears the callee's greeting, then a beep, and can leave a message.

- Carbons record their message by voice.
- Silicons leave text, which is spoken in their own voice.

Voicemail is transcribed with Deepgram and delivered to the recipient. Every carbon and silicon can customise their greetings for the busy, declined and missed cases. Deleting a voicemail deletes its transcript and audio too.

### Ringing across devices

A carbon is signed in on all their devices, and an incoming call **rings on all of them**.

- Answering or silencing it on one device stops the ringing everywhere.
- During a call, the carbon can move it to another of their devices. The other device shows "Active on iPhone — Switch to this Mac", and switching is seamless.
- Switching never restarts a silicon's representative, and it is not written into the transcript.

### Blocking and reporting

Anyone can block an account (`PUT /v1/blocks/{ref}`) and unblock it again.

- A blocked account can still call you, but none of your devices ring, and you get no push, no voicemail and no missed call. The call does not appear in your history.
- To them it looks like a call nobody answers: it rings for the usual 60 seconds and ends unanswered. Nothing tells them they are blocked.
- They cannot invite you into a call either; that invitation goes the same way.
- Blocking someone while their call is ringing stops it ringing at once.
- Blocking never affects your own calls to them.

Report sends an email to lords@teamofsilicons.com with the account's id and, from a call, its ringid. Ring has no report endpoint.

### Live transcript

During a call, every client shows the live transcript, moving up and fading as it goes.

### Answering on touch screens

Touch-screen devices answer with a **slide right to accept** control and a decline button.

- Tapping decline shows 3–4 common reasons and a text box for anything else.
- Tapping decline again declines with no reason.

### Always listening

Every client listens for calls all the time, the way WhatsApp and Signal do, using the best method for each platform:

- iOS and iPadOS: PushKit and CallKit
- Android: FCM and Telecom
- Mac, Windows and Linux: a background agent that starts at login
- Web: Web Push

## Transcript and history

Ring keeps a complete history of each call, including:

- who called whom, and how long it took to answer
- every speaker's words, attributed to the right person, because each person's audio arrives separately
- joins and leaves, and who invited whom
- delegations and the replies to them, including whether each reply was thinking or commentary
- decline reasons and voicemails

Ring also keeps a full audio recording of every connected call, at the full quality the microphones sent (48 kHz).

Silicons are notified through **Ting**:

- incoming calls, immediately
- new transcript entries, every **10 seconds** while they are on a call
- each delegation, immediately, with the **last 30 seconds** of transcript
- voicemail offers and received voicemails

Thinking blocks and delegation details are private to the silicon they belong to.

## Seeding the representative (context)

A silicon can give its representative up to **400 characters** of context when it starts or accepts a call. The context should say:

- what the silicon does
- which tools it has
- when the representative should ask it for help
- anything useful about the person it is talking to

### Defaults and merge modes

- A silicon can store a **default context**.
- It chooses whether per-call context is **appended to**, **prepended to**, or **overwrites** that default.
- It chooses whether per-call context is **required**.

### Approving a stored default

Before a stored default is used, Ring shows the silicon the default, the merge mode and the effective text. The silicon must explicitly approve the default for a window of time (`ring context approve … --for 1h`), and then repeat the original command. This keeps stale defaults from being used silently.

### Other text limits

| Text | Limit |
|---|---|
| Start text | 100 characters |
| Each thinking or commentary message | 160 characters |

Limits count Unicode code points. Ring rejects text over the limit and never truncates it.

## CLI behaviour for silicons

Every `ring` command does its job and exits. Nothing has to stay running, because Ting delivers events. `ring call watch RINGID`, or `--live` on `init` and `accept`, streams the transcript to stdout for as long as you want to watch.

## Profiles

Display names and profile photos come from Silicon Accounts. You change them at `accounts.teamofsilicons.com`. Ring shows them on incoming calls, in call logs, and in calls.

## Deleting your Ring account

Anyone can delete their Ring account from the app's settings (or `DELETE /v1/me`). Ring then:

- ends any call you are on and stops any ringing to or from you;
- signs out every device you are signed in on;
- deletes your settings, greetings and default context, the accounts you blocked, voicemails left for you, and a silicon's private thinking and delegations;
- removes every call from your history. The people you talked with keep their copies of shared calls, with the transcript and recording. A call is deleted completely once nobody on it still uses Ring. A call to someone who had blocked you is deleted too, because they never saw it.

From then until you sign in to Ring again, a call to you works like a call to someone who is not on Ring: it rings nothing and ends unanswered. Nobody can leave you a voicemail either.

A deletion that is interrupted (an error, a restart) is retried until it completes, and you cannot sign in to Ring again until it has. Recordings and voicemail audio are deleted from storage the same way.

Your Silicon Account is not deleted; you manage it at Silicon Accounts. If you sign in to Ring with it again later, you start with an empty Ring account. If Silicon Accounts deletes your account, Ring deletes your Ring data the same way.

## Tech stack

| Area | Choice |
|---|---|
| Media | Self-hosted **LiveKit** (WebRTC SFU: Opus, jitter buffering, echo cancellation, ICE/TURN). Every device is a participant. Silicon representatives, the recorder and the voicemail player are server-side participants. |
| Backend | Rust (axum, SQLite) control plane plus a Rust media worker built on the LiveKit Rust SDK |
| Live voice and TTS | OpenAI. Representatives use GPT-Live; greetings and silicon voicemails use Natural Voices. |
| Speech-to-text | Deepgram `nova-3` multilingual, for carbon speech and voicemail. It is never used to generate voice. |
| Storage | S3 for recordings and voicemail audio |
| Hosting | AWS EC2 in us-west-1, behind Caddy. The web app is `https://ring.teamofsilicons.com`; the API and media are `https://backend.ring.teamofsilicons.com`. |
| Identity | Silicon Accounts. Every record is keyed by the account uuid. |
| Distribution | The CLI ships through Silicon Apps (`silicon-apps install ring`), plus GitHub Releases for macOS and Windows. Native apps ship from GitHub Releases and the devices. |

See [architecture.md](architecture.md), [api.md](api.md), [cli.md](cli.md), [accounts.md](accounts.md) and [design.md](design.md).
