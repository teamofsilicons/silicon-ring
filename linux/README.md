# Ring for Linux

The native Linux app for Silicon Ring: Rust, GTK4 (`gtk4` 0.11) and libadwaita (0.9), on top of
`ring-core` (HTTP API, realtime WebSocket, LiveKit call audio through libwebrtc's PulseAudio/ALSA
device module with echo cancellation). App id `com.teamofsilicons.Ring`, binary `ring-desktop`.
Ring sends no telemetry.

This directory is its own Cargo workspace (not a member of the root workspace).

## What it does

- **Sign in** with Silicon Accounts in the system browser (`gtk::UriLauncher`, which goes through
  the OpenURI portal under Flatpak): PKCE plus a loopback redirect on `127.0.0.1:<random port>`
  (`ring_core::auth::LoopbackReceiver`), then `POST /v1/auth/carbon`. The session token is stored
  with the Secret Service through oo7 (GNOME Keyring, KWallet, or the secret portal when
  sandboxed), and falls back to `~/.config/com.teamofsilicons.Ring/session.json` (mode 0600)
  when no keyring is reachable.
- **Always listening.** While signed in the app holds itself open, so closing the window keeps
  the realtime connection (and ringing) alive. "Open at login" writes
  `~/.config/autostart/com.teamofsilicons.Ring.desktop` (`ring-desktop --background`), or asks the
  Background portal under Flatpak. Both are switches in Settings.
- **Incoming calls** open an urgent ringing window (the ink "moment" screen; on X11 the urgency
  hint is set) with Accept, Decline (first press reveals reasons and a text box, second press
  declines without one) and Silence, play `ringtone.wav`, and post a notification with Answer and
  Decline: through the XDG Notification portal (`call.incoming` category, urgent priority, call
  button purposes) with `gio::Notification` as the fallback. Both use the same app actions.
- **Calls**: the dialer looks people up live (`GET /v1/accounts/{ref}`) and lists recent people;
  the outgoing screen plays `ringback.wav`; the call screen shows a timer, participants with a
  speaking pulse (from LiveKit active speakers matched to `media_identity`), the live transcript
  (final and interim lines moving up and fading), mute with a level meter, the microphone and
  speaker in use (see Limitations), add person, and hang up. Voicemail recording shows "Leave a message after the beep"
  and a Done button. Calls on another of your devices show "On a call with … on iPhone · Switch to
  this computer" (`POST /calls/{ringid}/media`); a call moved away shows "Moved to …".
- **Recents** (call details with a timeline, decline reasons, the transcript and the recording),
  **Voicemail** (unread dots, inline playback, a detail dialog with a scrubber and Call back) and
  **Settings** (account, this device's name and "Ring on this device", devices with sign-out,
  voicemail and greetings, appearance, open at login, keep running in the background, about,
  sign out). Recordings and voicemail are downloaded to a private cache file and played with
  `gtk::MediaFile`.

The look follows `udd/design.md`: Ring's tokens (warm paper and ink, the blue accent, beige
selection, status colours) are fed into libadwaita's own colour slots, so every widget keeps its
native behaviour; the brand fonts are loaded app-locally through fontconfig
(`FcConfigAppFontAddDir`) and used for UI text (BDO Grotesk), names and titles (Hedvig Letters
Serif) and pixel labels and timers (Geist Pixel Square).

## Layout

| Path | What |
|---|---|
| `src/main.rs` | GApplication setup and command-line options |
| `src/app.rs` | The controller: session, realtime events, call state, media |
| `src/runtime.rs` | The Tokio runtime and the bridge to the GLib main loop |
| `src/ui/` | Screens: window, home, recents, voicemail, settings, call, incoming, dialer, details, player |
| `src/style.rs`, `src/fonts.rs` | Design tokens and app-local fonts |
| `src/secret.rs`, `src/notify.rs`, `src/autostart.rs`, `src/sound.rs` | Platform integration |
| `data/` | Desktop entry, AppStream metainfo, D-Bus service, the symbolic icon |
| `package.sh` | Builds a `.deb` and an AppImage |
| `docker/Dockerfile` | A build environment (Debian trixie, GTK 4.18, clang 21) for building from macOS |

Fonts, sounds and the app icon come from the repository's `assets/` and are copied in by
`package.sh`. A development build finds them in the repository instead.

## Building

Requirements: Rust 1.98, GTK ≥ 4.14 and libadwaita ≥ 1.5 development files, clang and lld 21 or
newer (libwebrtc ships Chromium's hermetic libc++, which webrtc-sys only accepts from a recent
clang; `.cargo/config.toml` links with `clang -fuse-ld=lld`, because GNU ld takes many minutes
to link libwebrtc). On Ubuntu 24.04:

```sh
sudo apt install pkg-config libgtk-4-dev libadwaita-1-dev libdbus-1-dev libpulse-dev \
  libgtk-4-media-gstreamer gstreamer1.0-plugins-good
# clang/lld 21 from apt.llvm.org, then make `clang`, `clang++` and `ld.lld` point at them
cd linux && cargo build --release
```

From macOS, build inside colima (arm64 Linux):

```sh
docker build -t ring-linux-build linux/docker
docker run --rm -v "$PWD":/src -v ring-linux-cargo:/usr/local/cargo/registry \
  -v ring-linux-target:/target -e CARGO_TARGET_DIR=/target -w /src/linux \
  ring-linux-build cargo build --release
```

## Running

```sh
ring-desktop                    # production API
RING_API_URL=http://192.168.29.75:8765 ring-desktop   # a development ring-server
ring-desktop --api-url URL      # same, as an option
ring-desktop --background       # start hidden (what autostart runs); exits if not signed in
ring-desktop --new-call         # open the dialer
ring-desktop --audio-devices    # list the microphones and speakers libwebrtc can open
RUST_LOG=ring_desktop=debug ring-desktop
```

Under Xvfb, set `GSK_RENDERER=cairo` to take screenshots (`xwd` cannot see GL-rendered frames
there). Logs go to stderr only and never contain tokens or conversation text. Preferences live in
`~/.config/com.teamofsilicons.Ring/settings.json`; downloaded voicemail and recording audio is
cached in `~/.cache/com.teamofsilicons.Ring/audio` only while it plays.

## Packaging

```sh
linux/package.sh                # cargo build --release, then .deb (+ AppImage when a tool is found)
linux/package.sh --no-build     # package an existing build
```

Outputs go to `linux/dist/`. The `.deb` is built with `dpkg-deb` and gets its library
dependencies from `dpkg-shlibdeps`. The AppImage bundles GTK4 and libadwaita with `linuxdeploy`
and `linuxdeploy-plugin-gtk`, then packs it with `appimagetool` (set `LINUXDEPLOY`,
`LINUXDEPLOY_PLUGIN_GTK` and `APPIMAGETOOL`; `patchelf` must be installed). `package.sh` puts
GTK's, libadwaita's and AppStream's libraries back unmodified after linuxdeploy, because its
patchelf pass moves the resources they embed and GTK then aborts at start-up. With
`appimagetool` alone it builds an AppImage that uses the system's GTK4 and libadwaita.

`.github/workflows/linux.yml` builds, starts the app headless once (with a virtual PulseAudio for
`--audio-devices`), packages both formats and starts the AppImage, on `ubuntu-24.04` (x86_64)
and `ubuntu-24.04-arm` (aarch64).

Installed files: `/usr/bin/ring-desktop`, the desktop entry, metainfo, D-Bus service, hicolor
icons, and `/usr/share/com.teamofsilicons.Ring/{fonts,sounds}`.

## Limitations

- **Audio devices.** Calls use the system's default microphone and speaker. WebRTC's PulseAudio
  and ALSA device modules give devices no id, and LiveKit's `PlatformAudio` selects devices only
  by id, so the in-call Audio menu shows the defaults and points to the system sound settings
  (PipeWire and PulseAudio can also move Ring's streams there while a call runs). If a backend
  ever reports ids, the same menu becomes a picker.
- **Always on top.** GTK4 cannot keep a window above others, so the ringing window asks for
  attention instead (the X11 urgency hint; on Wayland the compositor decides). The notification's
  Answer and Decline buttons work either way.
- **AppImage sounds.** The AppImage bundles GTK4, libadwaita and the GStreamer core libraries but
  not GStreamer's plugins, so ringtones play through the host's plugins when they are installed.
