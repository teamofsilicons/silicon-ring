# Silicon Ring native apps

The Tauri 2 shell packages the shared SolidJS frontend for macOS, Windows, Linux, iOS, and Android. Mobile audio and call delivery run in a Swift/Kotlin plugin independently of the webview. The backend WebSocket URL is selected on the sign-in screen; production builds should use `wss://backend.ring.teamofsilicons.com/ws`.

## Desktop

Install [Tauri's platform prerequisites](https://v2.tauri.app/start/prerequisites/) and a current stable Rust toolchain, then run from the repository root:

```sh
npm ci --prefix web
npm --prefix web run native:dev
npm --prefix web run native:build
```

Packages appear in `native/target/release/bundle/`. The desktop app uses the OS webview microphone implementation, the shared 24 kHz PCM worklet, a tray icon, and close-to-tray behavior. macOS microphone usage and audio-input entitlements are included. Distribution signing/notarization requires the team's certificates. The build workflow creates development packages for all three desktop platforms on both arm64 and x86_64 runners.

## iOS

The Xcode project is checked in under `gen/apple/`. Install Xcode, CocoaPods, the iOS platform SDK, and a simulator runtime for interactive testing. Install `aarch64-apple-ios-sim` and `aarch64-apple-ios` Rust targets as appropriate.

```sh
cd native
../web/node_modules/.bin/tauri ios build --debug --target aarch64-sim --no-sign --ci
```

For a simulator app on a Mac with SDKs but no bootable runtime, keep this command running in one terminal:

```sh
../web/node_modules/.bin/tauri ios build --open --debug --target aarch64-sim --no-sign --ci
```

Build in a second terminal from `native/`:

```sh
xcodebuild -project gen/apple/silicon-ring-native.xcodeproj \
  -scheme silicon-ring-native_iOS -sdk iphonesimulator \
  -destination 'generic/platform=iOS Simulator' -configuration debug \
  -derivedDataPath gen/apple/build/simulator CODE_SIGNING_ALLOWED=NO build
```

Tauri's CLI must remain running because the Xcode build script reads its build options. The resulting simulator application is `gen/apple/build/simulator/Build/Products/debug-iphonesimulator/Silicon Ring.app`.

The Swift plugin registers PushKit VoIP tokens, reports incoming calls through CallKit, handles answer/end/mute, maintains an authenticated WebSocket independently of the webview, and captures/plays 24 kHz mono PCM through AVAudioEngine. CallKit owns audio-session activation for calls. Session credentials live in an `AfterFirstUnlockThisDeviceOnly` Keychain item. Background modes `audio` and `voip` and the development APNs entitlement are included.

For a physical device or App Store distribution, select the Apple development team, enable Push Notifications for bundle ID `com.teamofsilicons.ring`, and use the corresponding provisioning profile. Change the entitlement to `production` for distribution. Configure the backend with `RING_APNS_KEY_FILE` (or `RING_APNS_KEY_BASE64`), `RING_APNS_KEY_ID`, `RING_APNS_TEAM_ID`, `RING_APNS_BUNDLE_ID`, and `RING_APNS_SANDBOX=true` for development profiles. The server uses the `.voip` topic.

## Android

Install Android Studio, SDK platform 36, build tools, command-line tools, and NDK. Set `JAVA_HOME`, `ANDROID_HOME`, and `NDK_HOME` as described in the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/#android). Then:

```sh
cd native
# Only required if gen/android has not been generated yet:
../web/node_modules/.bin/tauri android init --ci --skip-targets-install
../web/node_modules/.bin/tauri android build --debug --target aarch64 --apk --ci
```

Development APKs appear in `gen/android/app/build/outputs/apk/`. Release APKs/AABs need the team's signing keystore.

The Kotlin plugin implements a self-managed Telecom `ConnectionService`, incoming call notification actions, a foreground call service, ringtone, native AudioRecord/AudioTrack PCM, an independent authenticated WebSocket, FCM registration, and FirebaseMessagingService cold-start delivery. Session credentials are encrypted with an Android Keystore AES-GCM key. The app requests microphone, notifications, and self-managed call permissions as applicable.

FCM requires this app's Firebase Android configuration. Add `google-services.json` to `gen/android/app/` and enable the Google Services Gradle plugin in the generated root/app project, or provide equivalent `google_app_id`, `gcm_defaultSenderId`, `google_api_key`, and `project_id` resources. The JSON is ignored by Git. Configure the backend's `RING_FCM_SERVICE_ACCOUNT_FILE` or `RING_FCM_SERVICE_ACCOUNT_BASE64`. Allow incoming-call/full-screen notifications in Android settings when the OS requires it.

## Verification and remaining device checks

`npm test --prefix web` verifies identity input, explicit offline errors, transcript revision replacement, and PCM framing from 24/44.1/48 kHz source audio. The native workflow builds complete packages; successful compilation is separate from hardware behavior.

Before distributing mobile releases, verify on provisioned physical devices: foreground/background/terminated delivery; lock-screen answer and decline; microphone permission denied/granted; mute and Bluetooth/earpiece/speaker routing; call interrupted by a cellular call; network loss and recovery; actor-wide cancellation after answer elsewhere; handoff; private voicemail recording. APNs/FCM delivery and those hardware paths require the team's credentials and devices. A desktop build or simulator compilation does not establish them.
