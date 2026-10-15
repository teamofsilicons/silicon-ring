# Ring for Android

Kotlin + Jetpack Compose, one module (`:app`), `com.teamofsilicons.ring`, minSdk 26, target 36.
Calls ring through FCM high-priority data messages and Jetpack Telecom (`core-telecom`), with a
`phoneCall` foreground service and a CallStyle full-screen notification. Media is LiveKit Android
with Telecom owning audio routing. Ring sends no telemetry: only `firebase-messaging` is linked and
every Firebase collection switch is off in the manifest.

## Build

```sh
export JAVA_HOME="/Applications/Android Studio.app/Contents/jbr/Contents/Home"
export ANDROID_HOME=~/Library/Android/sdk
cd android
./gradlew assembleDebug assembleRelease
```

- `secrets/google-services.json` is copied to `app/google-services.json` (gitignored) when Gradle
  configures the project. Without it the app builds with push disabled.
- Release signing reuses the existing Ring key so installed APKs update in place. Gradle reads
  `secrets/silicon-ring-android-signing.private.json` and `secrets/silicon-ring-android-release.p12`,
  or, in CI, `RING_ANDROID_KEYSTORE_PATH`, `RING_ANDROID_KEY_ALIAS`, `RING_ANDROID_KEYSTORE_PASSWORD`
  and `RING_ANDROID_KEY_PASSWORD`. Check the result with
  `apksigner verify --print-certs app/build/outputs/apk/release/app-release.apk`; the certificate
  SHA-256 must be `ed087dc3…80ce` (`deploy/android-release-certificate.sha256` in the old tree).
- Brand fonts and sounds come from `../assets` through the generated `syncBrandResources` task.

Outputs: `app/build/outputs/apk/debug/app-debug.apk` and `app/build/outputs/apk/release/app-release.apk`.

## Servers

| Build | Default API | Cleartext |
|---|---|---|
| debug | `http://192.168.29.75:8765` (the dev Mac on the LAN) | allowed only for `192.168.29.75`, `10.0.2.2`, `127.0.0.1`, `localhost` |
| release | `https://backend.ring.teamofsilicons.com` | never |

Change it in Settings > About > Server, or on the sign-in screen by tapping the Ring mark five
times. A session belongs to one server, so changing it signs out.

## On a physical phone

1. Enable USB debugging, then `adb install -r app/build/outputs/apk/debug/app-debug.apk`
   (or the release APK; uninstall first when switching between debug and release signatures).
2. For the debug build, run the dev stack on the Mac (`livekit-server --config secrets/livekit.dev.yaml`,
   ring-server and ring-media with `secrets/dev.env`) and keep the phone on the same Wi-Fi as
   `192.168.29.75`.
3. Sign in with Silicon Accounts as a carbon. On the home screen, allow notifications and the
   microphone, and if the "Set up calling" card asks for it, allow full-screen notifications
   (Android 14+: Settings > Apps > Ring > Notifications > Full-screen notifications).
4. Some vendors (Xiaomi, OPPO, Samsung) stop FCM for apps they put to sleep: set Ring's battery use
   to Unrestricted.
5. Call it from a silicon: `silicon-accounts login --app ring -q | ring login --slt-stdin --api-url http://192.168.29.75:8765`
   then `ring call init c:<you> --api-url http://192.168.29.75:8765`. With the screen off the call
   should wake the phone into the incoming-call screen; unlocked, it shows as a heads-up CallStyle
   notification.

Useful while testing:

```sh
adb logcat -s RingCalls RingTelecom RingCallService RingMedia RingRealtime RingPush RingSounds
adb shell dumpsys telecom | grep 'Call id='
adb shell dumpsys notification --noredact | grep -A3 com.teamofsilicons.ring
```

## Debug-only design catalog

Debug builds include `DesignCatalogActivity`, which renders screens with sample data and can
exercise the call stack without a server:

```sh
adb shell am start -n com.teamofsilicons.ring/.debug.DesignCatalogActivity --es screen incoming --ez dark false
# screens: incoming, incoming-reasons, calling, ringing, active, voicemail, ended, recents, voicemails
adb shell am start -n com.teamofsilicons.ring/.debug.DesignCatalogActivity --es screen simulate-incoming
# TOKEN: a join token for the dev SFU, e.g. from `lk token create --join --room test --identity carbon:test:dev_x`
adb shell am start -n com.teamofsilicons.ring/.debug.DesignCatalogActivity --es screen media-test \
    --es url ws://192.168.29.75:7880 --es token "$TOKEN"
```

`simulate-incoming` rings a local call through Telecom, the call service and the CallStyle
notification. `media-test` joins a LiveKit room with the app's media stack and publishes the mic.
