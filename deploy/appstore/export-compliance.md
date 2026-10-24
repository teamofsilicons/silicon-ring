# Export compliance (encryption)

## Recommendation

Keep `ITSAppUsesNonExemptEncryption` = `false`. **It is in `apple/iOS/Info.plist`** (outside the `RING_DEBUG` block, so Release builds have it) and in `apple/macOS/Info.plist`, so nothing needs adding. With it, App Store Connect does not ask the encryption questions for each build, and no documentation needs uploading. Check it is still there after this round's Info.plist fixes: `plutil -p` on the archived app's `Info.plist` should show `"ITSAppUsesNonExemptEncryption" => false`.

This is a reading of the rules for a standard case, not legal advice. If Team of Silicons has export counsel, they have the final word.

## What Ring uses

| Where | What | Algorithms | Whose implementation |
|---|---|---|---|
| API and realtime (`apple/RingKit/Networking/APIClient.swift`, `RealtimeConnection.swift`) | HTTPS and WSS to `backend.ring.teamofsilicons.com` | TLS 1.2/1.3 | Apple's (URLSession, URLSessionWebSocketTask) |
| Sign-in (`apple/RingKit/Auth/SignIn.swift`) | Silicon Accounts in `ASWebAuthenticationSession`; a SHA-256 PKCE challenge | TLS; SHA-256 (hashing, not encryption) | Apple's (Safari, CryptoKit) |
| Session storage (`apple/RingKit/Auth/SessionStore.swift`) | The session token in the Keychain | — | Apple's |
| Call media (`apple/RingKit/Calls/CallMedia.swift`, LiveKit Swift 2.17.0) | Voice over WebRTC to Ring's self-hosted LiveKit server; signalling over WSS | DTLS-SRTP (IETF RFC 5764): DTLS 1.2 key exchange, SRTP with AES | libwebrtc's BoringSSL, inside LiveKit's open-source `LiveKitWebRTC.xcframework` |

Ring has no proprietary or non-standard cryptography and no end-to-end encryption feature of its own (LiveKit's optional E2EE is not used). All server-side encryption (AES-256-GCM sealing of Accounts refresh tokens, S3 encryption at rest) runs on the server, not in the app.

This round adds no cryptography to the app. Account deletion and Block are ordinary requests over the same HTTPS connection, Report hands a prefilled email to the user's mail app, and the recording notice is a screen. The Deepgram `mip_opt_out` change is a server-side request parameter. So the answer stays `false`.

## Why `false` is defensible

- Every algorithm is a published international standard (IETF TLS, DTLS, SRTP), used only to protect the app's own communication, not offered to the user as a security feature.
- Ring is free, sold to anyone, and its encryption cannot be changed by the user: a mass-market item (EAR 5D992.c, licence exception ENC §740.17(b)(1)). Since BIS's 2021 rule, mass-market software that is not a "component" needs no self-classification report.
- The whole app is built from publicly available open source (Ring under MIT, LiveKit under Apache 2.0, WebRTC under BSD) using only standard cryptography, which the EAR treats as not subject to it (§734.3(b)(3), §742.15(b)).

## If App Store Connect asks anyway

It asks only when the key is missing from a build, or under App Information → App Encryption Documentation. The answers that match the facts above:

| Question | Answer |
|---|---|
| Does your app use encryption? (Select Yes even if it only uses Apple's) | **Yes** |
| Does your app qualify for any of the exemptions in Category 5, Part 2 of the EAR? | **Yes**: standard encryption only, mass market, publicly available |
| What type of encryption algorithms does your app implement? (newer form) | The accurate choice is **"Standard encryption algorithms instead of, or in addition to, using or accessing the encryption within Apple's operating system"**, because WebRTC's DTLS-SRTP is BoringSSL rather than Apple's. That form then asks whether the app will be on the App Store in France; see below. |

## The conservative alternative, and France

If you would rather declare the WebRTC encryption as non-exempt:

1. Change `ITSAppUsesNonExemptEncryption` to `true` in `apple/iOS/Info.plist` (Claude can do it).
2. In App Store Connect, choose "Standard encryption algorithms…".
3. For France, either upload a French encryption declaration (ANSSI) or leave France out of the app's availability.

Many WebRTC calling apps ship with `false` on the reasoning above; the France question is the only point where the two readings differ.
