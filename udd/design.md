# Ring design language and screens

Every client follows Silicon UI ([ui.teamofsilicons.com](https://ui.teamofsilicons.com)).
- **Web:** uses Silicon UI natively (`@teamofsilicons/silicon-ui`).
- **Native apps:** carry the same tokens into platform-native controls. Each app should feel native to its platform and still recognisably Ring: calm, monochrome, hairline borders, soft shadows, springy and restrained motion. Colour is used only for status.

## Tokens (mirror these as named colours with light and dark variants)

| Token | Light | Dark |
|---|---|---|
| background | #FFFFFF | #141414 |
| surface | #FFFFFF | #191919 |
| surfaceRaised | #FFFFFF | #1F1F1F |
| surfaceMuted | #F8F8F8 | #242424 |
| foreground | #0B0B0B | #F2F2F2 |
| textSecondary | #585858 | #B4B4B4 |
| textMuted | #7D7D7D | #8C8C8C |
| border | #E9E9E9 | #2B2B2B |
| borderSubtle | #F3F3F3 | #222222 |
| borderStrong | #C4C4C4 | #484848 |
| primary (fill) / onPrimary | #0B0B0B / #FFFFFF | #F2F2F2 / #141414 |
| success (accept) | #007B12 | #4CD676 |
| danger (decline / hang up) | #CD1526 | #FF736D |
| warning | #B66400 | #FFB90E |
| siliconGradient | #E2D8F8 → #FADFC6 (100°) | #463C59 → #5A422F |
| siliconInk (text on gradient) | #644C8A → #8A542D | — |

- **Spacing:** 2, 4, 8, 12, 16, 20, 24, 32, 48, 64, 80.
- **Radii:** detail 2, control 6, container 8, overlay 12. Use continuous/squircle corners where the platform supports them. Avatars, dots and pills are full circles or capsules.
- **Control heights:** 36, 44 (default touch), 50.
- **Avatar sizes:** 28, 36, 48, 88 (incoming and in-call caller), 120 (full-screen incoming on phones).
- **Type:** Inter for body and Geist for display, both under the OFL. You may use the platform system font instead (SF Pro, Roboto, Segoe UI Variable, Cantarell) at the same sizes.

  | Use | Values |
  |---|---|
  | Sizes | 12, 14 (default UI), 16, 18, 22, 28, 36, 52 |
  | Weights | 400 body, 500 controls and titles, 600 page headings |
  | Tracking | −1% body, −3% display |

- **Shadows:**

  | Level | Light | Dark |
  |---|---|---|
  | raised | y 6, blur 18, black 6.5% | 22% |
  | floating | y 20, blur 48, black 10.5% | 34% |

- **Scrim:** black 46% with a 7px blur.
- **Motion:**
  - Durations: 120, 160, 240, 480 ms.
  - Springs:
    - snappy: 0.26 s, bounce 0.12
    - smooth: 0.4 s, no bounce
    - morph: 0.42 s, bounce 0.16
  - Press scale: 0.97.
  - Respect Reduce Motion by using fades only.
- **Icons:** lucide at stroke 1.75 (web), or the platform's symbols at a similar weight (SF Symbols, Material Symbols Rounded, Segoe Fluent Icons, Adwaita symbolic). The ones Ring uses:
  - phone, phone-incoming, phone-outgoing, phone-missed, phone-off
  - mic, mic-off, volume-2
  - voicemail, user-plus, users
  - bot (silicon badge)
  - arrow-right-left (switch device)
  - history, settings, search, play, pause, grid

## Identity display

- **Name and id.** Show the display name large and the id (`c:alice`, `si:tos`) below it in textSecondary. If the display name is null, show the id large instead.
- **Silicon badge.** Mark silicons with a small "Silicon" badge on the silicon gradient, using siliconInk text and a bot icon.
- **Avatar.** Use the `pfp_url` image, or initials on surfaceMuted as a fallback.

## Screens (every carbon client)

### 1. Sign in
- Ring wordmark and a one-line description.
- A **Continue with Silicon Accounts** primary button. It opens hosted Accounts sign-in (see accounts.md).
- A footer line: "Silicons use the ring CLI".

### 2. Home
Phones use bottom tabs. Tablets and desktop use a sidebar. The tabs are:
- **Recents:** call history. Each row has an avatar, name, a direction icon, missed calls in danger colour, the time, and a duration or outcome. Tapping a row opens Call details. A call button on the row redials.
- **Voicemail:** a list with an unread dot, a transcript preview and an inline play control.
- **Settings.**

There is a prominent **New call** button: a floating action button on phones, a top bar button on desktop. It opens the dialer. The dialer has:
- a search field "Enter c:handle or si:handle"
- a live lookup result card (avatar, name, id, an "On Ring" or "Not on Ring yet" note)
- recent people

### 3. Outgoing call
- The callee's avatar (88), name and id.
- Status: "Calling…" (text shimmer), then "Ringing…".
- The caller plays the ringback locally ("tring tring, pause", 3 s cycle) until the call is answered or goes to voicemail.
- A hang-up button: a filled danger circle, 64 px.
- **Voicemail state:** the caller hears the greeting and beep from the server. The screen shows "Leave a message after the beep" and a "Done" button, which hangs up and sends the message.

### 4. Incoming call
- **iOS, iPadOS and Android, locked or in the background:** the system call UI (CallKit, Telecom/CallStyle).
- **In the app** (all platforms), a full-screen sheet with:
  - the caller's avatar (120 on phones, 88 on desktop), large name and id
  - "is calling you", plus "with Alice, Bob" if others are already on the call
  - **Touch devices:** a slide-to-accept track ("slide to answer", success colour) and a round decline button.
  - **Desktop and web:** Accept (success) and Decline (danger) buttons.
  - **Decline:** pressing it once reveals reasons, as chips, and a text box. Pressing decline again declines without a reason. The reasons are:
    - "Can't talk right now"
    - "In a meeting"
    - "Call you back soon"
    - "Text me instead"
  - A **Silence** text button.
- **Desktop** also shows a floating always-on-top ringing panel (macOS NSPanel, a Windows topmost window, a Linux urgent window) and a system notification with Answer and Decline actions.

### 5. In call
- **Header:** the call timer (tabular figures) and the participants. One remote participant gets a large avatar and name. Groups get an avatar grid that shows who is speaking with a soft ring pulse.
- **Live transcript:** fills the middle. Lines appear at the bottom, move up and fade out towards the top (a gradient mask). Each line shows the speaker's name in textSecondary, then the text. Interim text is shown in textMuted until final.
- **Controls** (round 56–64 px buttons):
  - Mute (toggles to a filled state)
  - Speaker or audio route (mobile)
  - Add person (opens the dialer; invites)
  - Hang up (filled danger)
- **Handoff banner:**
  - If this account's call is active on another device: "On a call with Alice on iPhone · **Switch to this Mac**".
  - On the device that lost the call: "Moved to Mac".

### 6. Call details
Participants, the outcome, decline reasons, a timeline of joins, leaves and invites, the full transcript, and a recording player once it is ready.

### 7. Voicemail detail
From, the reason ("Missed", "Declined: In a meeting", "Busy"), a player with a scrubber, the transcript, and Call back.

### 8. Settings
- **Account:** avatar, name and id from Silicon Accounts, with a link to "Edit profile on Silicon Accounts".
- **This device:** name, and a "Ring on this device" toggle.
- **Devices:** a list, with revoke.
- **Voicemail:** enabled, and the three greeting texts (busy, declined, missed), each with a reset.
- **Appearance:** system, light or dark.
- **Desktop only:** "Open at login" and "Keep running in the menu bar or tray".
- **About:** version, source and privacy ("Ring sends no telemetry").
- Sign out.

## Sounds (`assets/sounds`)

| File | What it is |
|---|---|
| `ringback.wav` | 3 s cycle: two 0.4 s tones (440 + 480 Hz) 0.2 s apart, then silence. Plays on the caller. |
| `ringtone.wav` | A pleasant marimba-like two-note pattern, 3 s cycle. Mobile apps can use the system ringtone instead. |
| `beep.wav` | 1 kHz, 0.4 s. Played by the server before voicemail recording. |
| `connected.wav`, `ended.wav` | Short soft ticks. |

## App icon (`assets/icon`)
A rounded square in the silicon gradient (lavender to peach), with a minimal near-black (#181818) phone handset whose ringing arcs suggest a signal. There are SVG sources and PNG exports at the standard sizes.
