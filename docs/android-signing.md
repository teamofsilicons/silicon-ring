# Android release signing

The Android CI job builds two artifacts: a development APK for isolated device tests, and a production APK signed with the persistent Ring release identity. The production APK is not debuggable and registers its FCM token with `push_environment=production`. Test-realm cold push is limited to development APKs with `push_environment=sandbox`.

The production signing certificate fingerprint is recorded in `deploy/android-release-certificate.sha256`. The private PKCS12 keystore and its credentials are kept in the ignored `secrets/` directory with mode `0600`. Preserve this identity securely: Android updates must retain the signing identity of the installed application. Do not regenerate it for each release.

Repository secrets used by `.github/workflows/native-apps.yml`:

- `RING_ANDROID_KEYSTORE_BASE64`: the base64-encoded PKCS12 keystore.
- `RING_ANDROID_KEYSTORE_PASSWORD`: the keystore password.
- `RING_ANDROID_KEY_PASSWORD`: the `ring` alias password.
- `RING_ANDROID_GOOGLE_SERVICES_JSON`: the Firebase client configuration for `com.teamofsilicons.ring`.

CI writes the keystore into the runner's temporary directory, sets `RING_ANDROID_KEYSTORE_PATH` and the password environment variables for the release build, verifies the APK signature and non-debug manifest, uploads `silicon-ring-android-aarch64-release`, and removes the temporary key. Forks without signing secrets build only the development APK. A production build with signing secrets requires Firebase configuration.

For local release builds, provide the same three signing environment variables, then run this from `native/`:

```sh
node ../web/node_modules/@tauri-apps/cli/tauri.js android build --target aarch64 --apk --ci
```

The APK appears under `native/gen/android/app/build/outputs/apk/universal/release/`. Verify it with Android SDK `apksigner verify --verbose --print-certs`, compare the signer SHA-256 digest with the recorded fingerprint, and confirm `aapt dump badging` does not include `application-debuggable` before publishing. Increase the application version for later releases.

Development and production signatures differ. Testing a production APK over a development installation requires removing that development installation first; this clears its local test session. Public users should receive only the production artifact.
