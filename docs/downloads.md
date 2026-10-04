# Native app downloads

Download [Silicon Ring v0.1.5](https://github.com/teamofsilicons/silicon-ring/releases/tag/v0.1.5) for your operating system and processor. **arm64** means Apple silicon, Windows on ARM, ARM64 Linux, or an ARM64 Android device; **x86_64** means 64-bit Intel/AMD.

| Platform | Processor | Installer |
| --- | --- | --- |
| macOS | Apple silicon / arm64 | [silicon-ring-0.1.5-macos-arm64.dmg](https://github.com/teamofsilicons/silicon-ring/releases/download/v0.1.5/silicon-ring-0.1.5-macos-arm64.dmg) |
| macOS | Intel / x86_64 | [silicon-ring-0.1.5-macos-x86_64.dmg](https://github.com/teamofsilicons/silicon-ring/releases/download/v0.1.5/silicon-ring-0.1.5-macos-x86_64.dmg) |
| Windows | arm64 | [EXE setup](https://github.com/teamofsilicons/silicon-ring/releases/download/v0.1.5/silicon-ring-0.1.5-windows-arm64-setup.exe) · [MSI](https://github.com/teamofsilicons/silicon-ring/releases/download/v0.1.5/silicon-ring-0.1.5-windows-arm64.msi) |
| Windows | x86_64 | [EXE setup](https://github.com/teamofsilicons/silicon-ring/releases/download/v0.1.5/silicon-ring-0.1.5-windows-x86_64-setup.exe) · [MSI](https://github.com/teamofsilicons/silicon-ring/releases/download/v0.1.5/silicon-ring-0.1.5-windows-x86_64.msi) |
| Linux | arm64 | [AppImage](https://github.com/teamofsilicons/silicon-ring/releases/download/v0.1.5/silicon-ring-0.1.5-linux-arm64.AppImage) · [DEB](https://github.com/teamofsilicons/silicon-ring/releases/download/v0.1.5/silicon-ring-0.1.5-linux-arm64.deb) · [RPM](https://github.com/teamofsilicons/silicon-ring/releases/download/v0.1.5/silicon-ring-0.1.5-linux-arm64.rpm) |
| Linux | x86_64 | [AppImage](https://github.com/teamofsilicons/silicon-ring/releases/download/v0.1.5/silicon-ring-0.1.5-linux-x86_64.AppImage) · [DEB](https://github.com/teamofsilicons/silicon-ring/releases/download/v0.1.5/silicon-ring-0.1.5-linux-x86_64.deb) · [RPM](https://github.com/teamofsilicons/silicon-ring/releases/download/v0.1.5/silicon-ring-0.1.5-linux-x86_64.rpm) |
| Android 8.0+ | arm64 | [silicon-ring-0.1.5-android-arm64.apk](https://github.com/teamofsilicons/silicon-ring/releases/download/v0.1.5/silicon-ring-0.1.5-android-arm64.apk) |

The Windows filenames are `silicon-ring-0.1.5-windows-ARCH-setup.exe` and `silicon-ring-0.1.5-windows-ARCH.msi`. Linux filenames are `silicon-ring-0.1.5-linux-ARCH.AppImage`, `.deb`, and `.rpm`, with `ARCH` set to `arm64` or `x86_64`.

## Install

**macOS:** Open the DMG and copy Silicon Ring to Applications. The app targets macOS 12 or later. The macOS packages are signed with the publisher's Apple Developer ID certificate. They have not been notarized, so Gatekeeper may still require verification according to your organization's installation policy. The [web app](https://ring.teamofsilicons.com) is also available.

**Windows:** Choose either the EXE setup or MSI for your processor. These installers are not Authenticode signed, so Windows may display an unknown-publisher or SmartScreen prompt. The desktop app uses Microsoft WebView2. Use your organization's installation policy or the web app if unverified installers are not permitted.

**Linux:** Install the DEB or RPM with your distribution's package manager so its declared dependencies are resolved. To use the AppImage, mark it executable and run it. The CI packages were built on Ubuntu 24.04; compatibility with older distributions has not been established. AppImage support depends on the host's runtime/FUSE configuration. Linux packages are not package-signed.

**Android:** Install the arm64 APK on Android 8.0 or later (API 26+). Android may ask you to allow installation from the browser or file manager you use to open it. This is a production release build signed with the project's stable release certificate; future updates must use the same certificate. CI debug APKs use a different certificate and cannot replace this release as an update. The public certificate fingerprint is pinned in [android-release-certificate.sha256](https://github.com/teamofsilicons/silicon-ring/releases/download/v0.1.5/android-release-certificate.sha256).

On first use, choose **Continue as Carbon**. Ring opens IAM in your browser and returns to the app after sign-in and organization selection. An IAM-issued Ring token is also supported in connection settings. Allow microphone access when you intend to make calls. Installing an app does not create an account or bypass IAM approval.

## Verify the download

Download [SHA256SUMS-native](https://github.com/teamofsilicons/silicon-ring/releases/download/v0.1.5/SHA256SUMS-native) and [native-provenance.json](https://github.com/teamofsilicons/silicon-ring/releases/download/v0.1.5/native-provenance.json) alongside the installer. Calculate its SHA-256 and compare the entire value against its filename in the checksum file:

```sh
# macOS example
shasum -a 256 silicon-ring-0.1.5-macos-arm64.dmg
# Linux example
sha256sum silicon-ring-0.1.5-linux-x86_64.AppImage
```

```powershell
# Windows example
Get-FileHash .\silicon-ring-0.1.5-windows-x86_64-setup.exe -Algorithm SHA256
```

Checksums detect changed bytes; they are not publisher signatures. The provenance file records the source commit, workflow run, original artifact names, package sizes, and individual hashes. Use `native-provenance.json` for the exact source commit and workflow run for each asset. Public staging only renames files; installer bytes are unchanged.

For Android, verify both the APK checksum and its signing certificate. With Android SDK Build Tools installed, run:

```sh
apksigner verify --verbose --print-certs silicon-ring-0.1.5-android-arm64.apk
```

Verification must succeed. Compare the reported signer certificate SHA-256 digest with `android-release-certificate.sha256`, which contains the pinned 64-character lowercase hexadecimal digest of the DER certificate. The [Android signing guide](android-signing.md) describes the stable release key and build checks.

## Mobile and source builds

Development-signed iPhone apps and their provisioning profiles remain private to the authorized devices; there is no public iPhone installer in this release. See the [native guide](../native/README.md) for source builds, mobile setup, and verified device behavior.
