# Ring design language and screens

**Base style: teamofsilicons.com.** Ring looks like part of the Team of Silicons site, and its source is in `../www-tos` (`src/styles.css`, `src/components`, `src/pages/products.tsx`). That means:

- warm paper backgrounds and ink-black text
- one blue accent
- beige for selected and hover states
- 0.5 px hairline borders and soft shadows
- BDO Grotesk for UI text, Hedvig Letters Serif for display, Geist Pixel for small uppercase labels and the wordmark

How each client applies it:
- **Web:** uses Silicon UI components ([ui.teamofsilicons.com](https://ui.teamofsilicons.com), `@teamofsilicons/silicon-ui`). A product theme maps Silicon UI's tokens to the values below. Only token values change, which Silicon UI's rules allow.
- **Native apps:** carry the same tokens into platform-native controls, so each feels native to its platform while clearly being the same Ring.

## Tokens (named colours with light and dark variants)

The site is light-only. The dark theme is derived from the Ring mark itself: ink background with cream marks.

| Token | Light (teamofsilicons.com) | Dark |
|---|---|---|
| background | #FFFDF9 (warm paper) | #181818 (ink) |
| surface (cards, sheets, lists) | #FFFFFF | #1E1E1C |
| surfaceRaised (dialogs, popovers) | #FFFFFF | #252421 |
| surfaceMuted (grouped backgrounds, inputs) | #F6F2EA | #22211E |
| beige (selected, hover, chips, badges) | #EDE8E0 | #2E2C28 |
| foreground (primary text) | #1E1E1C | #EDE8E0 |
| ink (marks, strongest contrast) | #181818 | #F6F2EA |
| textSecondary | #4A4842 | #C9C3B6 |
| textMuted (labels, meta) | #6E6B63 | #9C978C |
| border | #E7E3D8 | #2F2D29 |
| borderStrong (control outlines) | #D9D2C4 | #3D3A35 |
| accent / primary (fill) | #1F5CB1 | #3872CA |
| accentHover | #184A94 | #4A80D2 |
| onPrimary | #FFFFFF | #FFFFFF |
| accentText (links on background) | #1F5CB1 | #8DB2EC |
| accentSubtle (silicon badge background) | rgba(31,92,177,0.08) | rgba(91,143,217,0.16) |
| accentBorder | rgba(31,92,177,0.25) | rgba(91,143,217,0.35) |
| success (accept, connected) | #1F7A4D | #4CC38A |
| danger (decline, hang up, missed) | #C8322B | #F2706A |
| warning | #B66400 | #F0B54A |

## Typography

The fonts are in `assets/fonts`, all under the OFL; see `assets/fonts/README.md` for file and PostScript names. Bundle them in every client.

| Role | Family | Use |
|---|---|---|
| UI / body | **BDO Grotesk** 400, 500, 700 | Everything by default. 14–16 px UI text, 500 for buttons and titles. Letter-spacing 0. |
| Display | **Hedvig Letters Serif** 400 | People's names on the incoming, outgoing and in-call screens; page titles ("Recents", "Voicemail"). Tight leading (1.1–1.2). |
| Labels | **Geist Pixel Square** | The wordmark **RING**; small uppercase labels (section headers, badges, call status such as "RINGING", "ON CALL", the call timer, meta lines). 11–13 px, tracking +2–4%. |

**Type scale (px):**

| Use | Size |
|---|---|
| Pixel labels | 11, 12, 13 |
| Body | 14, 15, 16 |
| Headings | 18, 22, 26 |
| Display names on call screens | 36 on phones, 44 on desktop |
| Hero | 52 |

**Line height:** 1.5 for body and 1.18 for serif display.

## Shape, depth and motion

| Element | Specification |
|---|---|
| Buttons and inputs | Radius 8. Primary is a filled blue accent with white text, weight 500, soft shadow (`0 4px 6px -1px rgba(0,0,0,.08), 0 2px 4px -2px rgba(0,0,0,.06)`) and a hairline accent border. Secondary is a white (70%) fill with border #E5E5E5 (light), a soft shadow and a backdrop blur. Small buttons are 28–32 px tall; default controls 36–44 px (44 minimum on touch). |
| Cards and list groups | Surface fill, radius 10–12, 0.5 px border, shadow `0 1px 4px rgba(0,0,0,.06)`. |
| Popovers, sheets and dialogs | Radius 12, 0.5 px borderStrong, shadow `0 12px 32px rgba(0,0,0,.10), 0 2px 6px rgba(0,0,0,.05)`. Scrim is ink at 40%. |
| Badges | Height 22, radius 5, 0.5 px border #D9D2C4 on beige, Geist Pixel Square 11 px uppercase. The **SILICON** badge uses accentSubtle with accentBorder and accent text (exactly like teamofsilicons.com's `.wt-badge--silicon`). The **CARBON** badge is beige. |
| Selected nav item and segmented controls | A beige pill indicator that slides between items, as in the site nav. |
| Hairlines | 0.5 px, or one physical pixel on native. Dividers use `border`. |
| Avatars | Circles. Fallback: the initial in Hedvig Letters Serif, accent-coloured, on beige with a 0.5 px border (as on the site). Sizes 28, 36, 48, 88, 120. |
| Focus | 2 px ink outline at 2 px offset (accent ring on buttons). |

**Spacing:** 4, 6, 8, 12, 16, 20, 24, 32, 48, 64.

**Motion:** 150 ms colour and border transitions, press scale 0.98, springs for sheets (smooth, no bounce) and the speaking-indicator ring pulse. Respect Reduce Motion by using fades only.

**Icons:** thin line icons at about 1.4–1.75 stroke (lucide on web; SF Symbols with light/regular weight, Material Symbols Outlined (weight 300), Segoe Fluent Icons, or Adwaita symbolic on native).

## Ring mark and app icon (`assets/icon`)

The Ring mark is concentric cream (#EDE8E0) rings around a cream dot on ink (#181818). It is the same mark as teamofsilicons.com/logos/ring.svg.

| File | Use |
|---|---|
| `icon.svg` | full bleed: iOS, web, Windows |
| `icon-macos.svg` | macOS rounded square |
| `foreground.svg` | Android adaptive foreground; the background colour is #181818 |
| `monochrome.svg` | tray, notification and themed icons |
| `ring-logo-800.png` | the original raster |
| `png/` | rendered sizes |

**Wordmark:** "RING" in Geist Pixel Square next to the mark.

**Signature motif:** the concentric rings also animate during calls. While ringing or connecting, faint rings expand from the avatar. While someone speaks, a ring pulses around their avatar.

## Identity display

- Show the display name large, in serif on call screens. Below it, show the id (`c:alice`, `si:tos`) in Geist Pixel Square, textMuted. If the display name is null, show the id large instead.
- Silicons get the **SILICON** badge. Avatars come from `pfp_url`, with the initial fallback above.

## Screens (every carbon client)

### 1. Sign in
- The Ring mark (88 px), the "RING" pixel wordmark, and one serif line: "Calls between carbons and silicons."
- A primary **Continue with Silicon Accounts** button that starts hosted Accounts sign-in (see accounts.md).
- A muted footer: "Silicons use the ring CLI · Ring sends no telemetry".

### 2. Home
- **Layout:** phones use a bottom tab bar; tablets and desktop use a sidebar on surfaceMuted with a beige selected pill.
- **Recents**
  - A serif page title.
  - Rows inside a card group: avatar, name, a SILICON badge where relevant, a direction icon, a pixel-font meta line (time · duration or outcome), and missed calls in danger colour.
  - Tapping a row opens Call details; a trailing call button redials.
  - Empty state: the faint Ring mark and "No calls yet".
- **Voicemail:** rows with an unread accent dot, the reason, a transcript preview and an inline play control.
- **Settings.**
- **New call** is a primary button: a floating action button on phones, a top-right button on desktop. It opens the **dialer**:
  - a field "c:handle or si:handle" with a live lookup result card (avatar, serif name, pixel id, an "ON RING" or "NOT ON RING YET" badge, and a Call button)
  - recent people below

### 3. Outgoing call
- Background: ink in dark mode, paper in light mode.
- Large avatar (120) with expanding faint rings, the serif name, and the pixel id.
- Pixel status: "CALLING…", then "RINGING".
- The ringback plays locally ("tring tring, pause", 3 s cycle).
- A hang-up button: a 64 px danger circle.
- Voicemail state: "LEAVE A MESSAGE AFTER THE BEEP" with a pulsing ring while recording, and a **Done** button that hangs up and sends.

### 4. Incoming call
- **System UI** on iOS, iPadOS and Android when locked or backgrounded (CallKit, Telecom/CallStyle).
- **In-app sheet:** ink background in both themes, which makes it a "moment" screen in the Ring mark's colours.
  - Avatar (120 on phones, 88 on desktop) with expanding rings, the cream serif name, and the pixel id.
  - "IS CALLING", plus "WITH ALICE, BOB" when others are already on the call.
  - **Touch devices:** a slide-to-answer track (cream thumb with a phone glyph on a translucent track, "slide to answer"), plus a round danger decline button.
  - **Desktop and web:** Accept (success) and Decline (danger) buttons.
  - **Decline:** pressing once reveals reason chips and a text box. The chips are "Can't talk right now", "In a meeting", "Call you back soon" and "Text me instead". Pressing decline again declines with no reason.
  - A **Silence** text button.
- **Desktop also shows:**
  - a floating always-on-top ringing panel: macOS NSPanel, a Windows topmost window, a Linux urgent window
  - a system notification with Answer and Decline actions

### 5. In call
- **Header:** the pixel-font call timer, and participants as large avatars with serif names. Whoever is speaking gets a soft concentric ring pulse.
- **Live transcript** fills the middle. Lines enter at the bottom, move up and fade toward the top behind a gradient mask. Each line shows a pixel-font speaker label in textMuted, then the text in BDO Grotesk 16–18. Interim text is muted until final.
- **Controls:** round 56–64 px buttons on surface or beige: mute (toggles to filled ink), speaker/route (mobile), add person (invite through the dialer), and hang up (filled danger).
- **Handoff banner (beige):**
  - On the device that doesn't have the call: "On a call with Alice on iPhone · **Switch to this Mac**".
  - On the device the call left: "Moved to Mac".

### 6. Call details
Participants, the outcome, decline reasons, a timeline in the style of the teamofsilicons.com writings timeline (hairline rail, dots, pixel-font times), the full transcript, and a recording player once it is ready.

### 7. Voicemail detail
From, the reason ("MISSED", "DECLINED · In a meeting", "BUSY"), a player with a scrubber, the transcript, and **Call back**.

### 8. Settings
Card groups:
- **Account:** avatar, name and id from Silicon Accounts, with "Edit profile on Silicon Accounts".
- **This device:** name, and a "Ring on this device" toggle.
- **Devices:** with revoke.
- **Voicemail:** enabled, plus the busy, declined and missed greetings, each with a reset.
- **Appearance:** system, light or dark.
- **Desktop only:** "Open at login", and "Keep running in the menu bar / tray".
- **About:** version, source, and "Ring sends no telemetry".
- **Sign out.**

## Sounds (`assets/sounds`)

| File | Description |
|---|---|
| `ringback.wav` | 3 s cycle: two 0.4 s 440+480 Hz tones 0.2 s apart, then silence. Played on the caller's side. |
| `ringtone.wav` | A soft marimba figure on a 3 s cycle. Mobile apps may use the system ringtone instead. |
| `beep.wav` | 1 kHz for 0.4 s. The server plays it before voicemail recording starts. |
| `connected.wav`, `ended.wav` | Short soft ticks. |
