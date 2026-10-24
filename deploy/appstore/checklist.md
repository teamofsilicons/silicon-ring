# App Store submission checklist: Silicon Ring for iPhone and iPad

Bundle id `com.teamofsilicons.ring`, version 1.0.0, team `LTBSK59BJ2` (individual account, so the store shows Shubham Gupta as the seller). Each step says who does it. Claude never deploys, signs in, creates accounts or handles keys; deploying is always the user's call.

## Files in this folder

| File | Use |
|---|---|
| `metadata.json` | Name, subtitle, promotional text, description, keywords, URLs, copyright, categories, age rating answers |
| `app-privacy.md` | App Privacy answers, with the code behind each one and the third parties |
| `export-compliance.md` | Encryption answers; `ITSAppUsesNonExemptEncryption = false` is set in `apple/iOS/Info.plist` |
| `review-notes.md` | The App Review notes to paste, the demo account, and the demo silicon |
| `screenshots/` | `capture.sh` and its output (see Screenshots) |

The privacy manifest is not in this folder: it ships from `apple/Resources/PrivacyInfo.xcprivacy` (row 9 below).

Field limits, checked with a script over `metadata.json` and `review-notes.md`: name 12/30 (fallbacks 24 and 28), subtitle 25/30, promotional text 145/170, description 1,960/4,000, keywords 98/100 (no spaces after commas), review notes 3,894/4,000 with the placeholders (about 3,900 once they are filled in with `si:ring-demo` and a typical Gmail address and password). `metadata.json` is valid JSON. If you edit the notes, keep them under 4,000 after filling them in.

## Done in this round, pending deploy

Nothing here reaches App Review until the server and web are deployed and a new build is uploaded. "Check" is what to look at on the TestFlight build or after the deploy.

| # | Done | Guideline | Check |
|---|---|---|---|
| 1 | **Account deletion.** `DELETE /v1/me` (`crates/ring-server/src/deletion.rs`), also run on the Silicon Accounts `account.deleted` webhook. In the app: Settings > Account > "Delete Ring account…", confirmed with "Delete Ring account"; on the web, Settings. Ring deletes only its own data; the screen says the Silicon Account is not deleted and links to Silicon Accounts (the user's decision: Silicon Accounts is a sign-in provider, like Sign in with Google) | 5.1.1(v) | Delete a throwaway account on TestFlight; it returns to sign-in, and signing in again starts empty |
| 2 | **Privacy policy and support pages**, `web/content/privacy.md` and `support.md`, rendered at `/privacy` and `/support`; contact lords@teamofsilicons.com | App Store Connect requires both URLs | Both open publicly at ring.teamofsilicons.com after the web deploy |
| 3 | **Web corrections**: the privacy policy and support text updated for this round (backups deleted from S3 after 35 days, the Deepgram opt-out, Block and Report, account deletion on iPhone, iPad and Mac) | 5.1.1(i) | Read `/privacy` once deployed; it must match `app-privacy.md` |
| 4 | **Recording and third-party AI notice**, "Ring records calls" (`apple/RingKit/Views/RecordingNoticeView.swift`, `AppModel+Calls.swift`). It names Deepgram and OpenAI and links the privacy policy. It shows once right after sign-in, unless the device agreed before; the microphone and notification prompts follow it. "Agree and continue" is remembered on the device. After "Not now" it comes back before placing, answering or moving a call there, and "Not now" then stops that call. A call answered from the system call screen before agreeing goes on, with the notice on its call screen | 5.1.2(i); call-recording consent laws | Fresh install, sign in: the notice, then the system prompts. "Not now", then place a call: the notice again. "Agree and continue": never again on that device, even after signing out and in |
| 5 | **Block and Report.** `GET /v1/blocks`, `PUT`/`DELETE /v1/blocks/{ref}` (`crates/ring-server/src/blocks.rs`); a blocked account's calls and invitations silently ring none of the blocker's devices. Report opens an email to lords@teamofsilicons.com with the account id and the ringid; the notes say reports are reviewed promptly, with no fixed time | 1.2 | Block a test account, call from it with the CLI: no device rings and the caller sees an unanswered call. Report opens Mail prefilled. Settings > Blocked lists and unblocks |
| 6 | **Server hardening** from this round's review, including the parts of account deletion and blocking above. VoIP cancel pushes now go only to the devices the incoming push reached (`push::cancel`, `rang`) | — | Comes with the server deploy |
| 7 | **Deepgram opt-out**: `mip_opt_out=true` on both requests (`crates/ring-media/src/stt.rs`) | Privacy policy accuracy | Comes with the ring-media deploy |
| 8 | **Backups expire after 35 days**: an S3 lifecycle rule on `backups/` only (`expire-database-backups`) on the `Assets` bucket in `deploy/stack.yaml`. It takes effect only once the stack change is applied: the user (lead) previews it with `deploy/deploy.py stack` and applies it with `deploy/deploy.py stack --execute` | Privacy policy accuracy | The change set touches only `Assets`; afterwards the bucket's lifecycle shows the rule |
| 9 | **Privacy manifest in the app**: `apple/Resources/PrivacyInfo.xcprivacy`, listed in the `RingApp` target template in `apple/project.yml`, so xcodegen bundles it in both the iOS and the Mac app. Nine data types, all linked, none for tracking; one required-reason API, `UserDefaults` (`CA92.1`) | ITMS-91053 rejects uploads without it | `PrivacyInfo.xcprivacy` is in the archived `.app`; the upload is accepted without an ITMS-91053 email |
| 10 | **Info.plist** (`apple/iOS/Info.plist`), done in code: `CFBundleDisplayName` "Silicon Ring"; `ITSAppUsesNonExemptEncryption` false; `UIBackgroundModes` only `audio` and `voip` (`remote-notification` removed); the "during development" `NSLocalNetworkUsageDescription` and the local ATS exceptions only in Debug (`#ifdef RING_DEBUG`, preprocessed) | 2.5.4; 2.3.1 | `plutil -p` on the archived app's `Info.plist`; the full name under the icon on the iPhone 13 mini; no local network prompt on TestFlight |
| 11 | **Server switch hidden in Release**, done in code: Release builds always use the production server (`ServerConfig.current`), and both long presses that open the switch (Settings > About > Version, and the Ring mark on the sign-in screen) are `#if DEBUG` only | 2.3.1 (hidden features) | Neither long press does anything on TestFlight |
| 12 | **Privacy policy and support links in the app**: Settings > About > "Privacy Policy" and "Support" (`AppInfo.privacyURL`, `supportURL`); the recording notice links the privacy policy too | 5.1.1(i) | Both rows open the pages |
| 13 | **The demo silicon**, `deploy/demo-silicon/`: `si:ring-demo` on the production host, polling the Ring API with the `ring` CLI. It answers calls from people (carbons) only and declines silicons with a reason; calls last at most 5 minutes; it answers up to 3 calls per caller an hour; 30 s after a call ends it calls the person back once, at most once every 10 minutes, and leaves a voicemail if that call back is declined or missed | 2.1 | Steps 5 and 9 below |
| 14 | **The listing** updated: copyright "2026 Team of Silicons", the description covers the iPhone and iPad app only, age rating 13+ with every answer, review notes for the decisions above with every label copied from the Swift source | 2.3.10, 2.3.6 | — |

If a row turns out not to have shipped in the uploaded build, take its sentence out of `review-notes.md` and `metadata.json` before submitting.

Still open, and not blocking:

- **OpenAI's data terms for GPT-Live.** Check that OpenAI's API data terms (no training by default, up to 30 days of abuse monitoring) cover it, and say so in the privacy policy. User.

## Deploys before review

The user's call, each time. Check that no call is in progress first.

| Deploy | Command |
|---|---|
| ring-server, ring-media and the web export, in one release (the server and web parts of rows 1 to 7) | Build as `docs/operations.md` says, then `deploy/deploy.py release --dry-run` and `deploy/deploy.py release` |
| The backup lifecycle rule (row 8) | `deploy/deploy.py stack` to preview the change set, then `deploy/deploy.py stack --execute` |
| The demo silicon (row 13) | Not part of a release: `deploy/demo-silicon/ship.py install` and `login` (step 5 below). A Ring release leaves it running |

## Your remaining steps, in order

| # | Step | Who |
|---|---|---|
| 1 | **Agreements.** App Store Connect → Business → Agreements: accept the latest Program License Agreement and the Free Apps agreement. A free app needs no tax or banking forms | User |
| 2 | **App Store Connect record.** Apps → + → New App: platform iOS, name **Silicon Ring** (fallbacks in `metadata.json`), primary language English (U.S.), bundle id `com.teamofsilicons.ring`, SKU `silicon-ring-ios`, Full Access | User |
| 3 | **API key.** Users and Access → Integrations → App Store Connect API → Team Keys → +, role **Admin** (Xcode's cloud-managed distribution certificate needs it). Download the `.p8` once and keep it outside the repository, for example `~/.appstoreconnect/private_keys/AuthKey_<KEY_ID>.p8`. Give Claude the Key ID, the Issuer ID and the path, never the file's contents | User |
| 4 | **Demo account, with a voicemail waiting.** Create a Gmail account for review only, 2-Step Verification off. Prepare it as `review-notes.md` ("The demo account") says: sign in to Ring once (any Ring app, or the web), set the name "App Review", leave it a voicemail and check it shows with its transcript, then sign out. While signed in, check two things: Sign in with Apple is offered on the Silicon Accounts sign-in page (guideline 4.8), and the "Delete your account" section is on `accounts.teamofsilicons.com/settings`, where the "Delete Ring account…" screen sends people (look; don't press it) | User |
| 5 | **Create and sign in the demo silicon,** as `deploy/demo-silicon/README.md` ("Set it up") says: check the id with `silicon-accounts id available si:ring-demo` (if it is taken, use another id, pass it to `ship.py login --silicon` and put it in the review notes); `silicon-accounts silicon create --id si:ring-demo --display-name "Ring Demo"` with yourself as custodian (keep the STK in your password manager, never in the repository); sign it in on your machine in `~/.ring-demo`; then `deploy/demo-silicon/ship.py install`, `deploy/demo-silicon/ship.py login` and `deploy/demo-silicon/ship.py status` | User |
| 6 | **DSA trader status.** Business → Trader status. If you declare yourself a trader, your address, phone and email are shown on EU storefronts; without any declaration the app is not distributed in the EU | User |
| 7 | **Availability.** Pricing and Availability: Free, all countries and regions **except China mainland** (CallKit apps are not allowed there) | User |
| 8 | **Build and upload** to TestFlight (see Build and upload), after the deploys above | Claude, once step 3 is done |
| 9 | **TestFlight pass, with the production push test.** Every earlier iOS build used the APNs sandbox; TestFlight and App Store builds use production APNs, which Ring has never sent a VoIP push through. Add yourself as an internal tester, install on the iPhone 13 mini and the iPad Air, lock both, and have the demo silicon or the CLI call you. Also confirm the APNs key in `silicon-ring/production` is enabled for production. Then the rest: the recording notice after sign-in and, after "Not now", before a call, a call to the demo silicon and its call back with the screen locked (the README's "Try it"), voicemail, moving a call between iPhone and iPad, Block and Report, account deletion on a throwaway account, both Debug long presses doing nothing, no local network prompt | User installs and answers; Claude places the calls and watches the logs |
| 10 | **IPv6 test.** App Review uses an IPv6-only network. On a Mac, Internet Sharing with "Create NAT64 Network", then join it from the iPhone and make a call. Ring's TURN server is named by host (`deploy/livekit.yaml.tmpl`), so calls should connect | User (it changes the Mac's network settings) |
| 11 | **Fill in the listing** (table below), then select the build on version 1.0.0 → Add for Review → Submit | User; Claude can fill the text fields through the API once there is a key |

## App Store Connect fields

| Where | What | Who |
|---|---|---|
| App Information | Subtitle; categories Productivity, then Social Networking; content rights No (Ring shows no licensed third-party content); age rating: the answers in `metadata.json`, then Override to Higher Age Rating → **13+** | User pastes, or Claude through the API |
| App Privacy | Privacy policy URL, then the data types in `app-privacy.md`. Publish it | User |
| Version 1.0.0 | Promotional text, description, keywords, support and marketing URLs, copyright "2026 Team of Silicons", screenshots | User pastes from `metadata.json`, or Claude through the API |
| App Review Information | Your name, phone and email (lords@teamofsilicons.com); Sign-In Information `{{DEMO_ACCOUNT}}` / `{{DEMO_PASSWORD}}`; the notes from `review-notes.md` with every placeholder filled in | User |
| Version release | **Manually release this version**, so it goes live only after you check it | User |

## Build and upload

| Step | Who |
|---|---|
| Bump `CURRENT_PROJECT_VERSION` in `apple/project.yml` (it is 4 now; every upload needs a new build number) | Claude |
| `cd apple && xcodegen`, archive `Ring-iOS` in Release for `generic/platform=iOS` | Claude |
| Export and upload with the API key: `xcodebuild -exportArchive` with `method` `app-store-connect` and `destination` `upload`, `-allowProvisioningUpdates` and `-authenticationKeyPath/-authenticationKeyID/-authenticationKeyIssuerID` (this also avoids Xcode's "No Accounts" problem on the command line) | Claude |
| No export compliance question should appear: the build says `ITSAppUsesNonExemptEncryption = false` (`export-compliance.md`) | — |

## Screenshots

App Store Connect needs one to ten screenshots per size, from real app screens with no real people's names or photos. `screenshots/capture.sh` captures them from Ring's Debug-only demo mode (`-RingDemo`), which fills the app with made-up people and calls and needs no server or sign-in. They are in `screenshots/` at accepted sizes: iPhone 6.9" 1320 × 2868 in `iphone-6.9/`, iPad 13" 2064 × 2752 in `ipad-13/`.

Who: the user looks through them before upload; Claude uploads them through the API, or the user drags them in. An app preview video is optional and not planned.

## After approval

| Step | Who |
|---|---|
| Release the version in App Store Connect | User |
| Replace the iPhone/iPad row in `README.md` and the website's download links with the App Store link | Claude |
| Keep the demo silicon running for every later review (`ship.py status`; renew its session with `ship.py login`), or stop it with `ship.py stop` and start it again before each submission | User |
