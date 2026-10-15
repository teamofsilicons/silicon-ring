# Ring for iPhone, iPad and Mac

Native SwiftUI apps for carbons. One xcodegen project builds two targets from shared code:

| Target | Platform | Notes |
|---|---|---|
| `Ring-iOS` | iPhone and iPad (iOS 17+) | PushKit VoIP → CallKit, LiveKit audio through the CallKit audio session, sidebar layout on iPad |
| `Ring-macOS` | Mac (macOS 14+, native SwiftUI + AppKit) | Menu bar + main window, login item, persistent WebSocket, floating ringing panel, time-sensitive notifications |

`RingKit/` is compiled into both: the API models (mirroring `udd/api.md`), the API client, the realtime WebSocket, sign-in (Silicon Accounts with PKCE), the LiveKit call media, the app model and every screen. Colours, fonts, the app icons and sounds come from `../assets` (see `udd/design.md`).

| Folder | Contents |
|---|---|
| `RingKit/Models` | Codable mirrors of the wire types, realtime frames and push payloads |
| `RingKit/Networking` | `APIClient` (async/await, Bearer token, error bodies) and `RealtimeConnection` (`/v1/ws` with ping, backoff, network and wake reconnects) |
| `RingKit/Auth` | PKCE sign-in through `WebAuthenticationSession`, Keychain session storage |
| `RingKit/Calls` | `CallMedia` (LiveKit 2.17.0), `CallSession`, the `CallSystem` protocol the platforms implement |
| `RingKit/App` | `AppModel`: session, history, rings, call flows, reconciliation |
| `RingKit/Views`, `RingKit/Design` | Every screen, the design tokens and components |
| `iOS/` | App entry, `CallKitBridge` (PushKit + CallKit), the route picker |
| `macOS/` | App entry, menu bar, `MacCallSystem` (ringing panel, notification, ringtone), login item |

## Build

```sh
cd apple
xcodegen                      # regenerates Ring.xcodeproj from project.yml

# iPhone and iPad
xcodebuild -project Ring.xcodeproj -scheme Ring-iOS -configuration Debug \
  -destination 'generic/platform=iOS' -allowProvisioningUpdates build

# Mac
xcodebuild -project Ring.xcodeproj -scheme Ring-macOS -configuration Debug \
  -destination 'platform=macOS' -allowProvisioningUpdates build
```

Install on a device (it must be unlocked):

```sh
xcrun devicectl device install app --device <udid> <DerivedData>/Build/Products/Debug-iphoneos/Ring.app
xcrun devicectl device process launch --device <udid> com.teamofsilicons.ring
```

## Servers

Debug builds talk to `http://192.168.29.75:8765` (the developer's LAN ring-server, allowed by a Debug-only ATS exception); Release builds talk to `https://backend.ring.teamofsilicons.com`. A hidden developer setting overrides it: long-press the version in Settings → About (or the Ring mark on the sign-in screen). Changing the server signs out, because sessions belong to the server that issued them.

## Signing and push

- Team `LTBSK59BJ2`, bundle id `com.teamofsilicons.ring` on both platforms, automatic signing.
- iOS reads the APNs environment from `embedded.mobileprovision` and registers both its VoIP and alert tokens with `PUT /v1/devices/current`.
- Mac Debug builds use `macOS/Ring-Debug.entitlements` (no push, no time-sensitive), so they sign without a provisioning profile: this Mac is not registered in the developer account, and registering it (`xcodebuild … -allowProvisioningDeviceRegistration`) is a change to the account that is left to its owner. Release builds use `macOS/Ring.entitlements` and need a Developer ID (or Mac development) profile with `aps-environment`; the Mac only gets APNs alert pushes with such a profile. While Ring runs, the Mac rings through its WebSocket, so push is only the fallback for when it is not running.

Ring sends no telemetry.
