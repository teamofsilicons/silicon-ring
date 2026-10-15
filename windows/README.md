# Ring for Windows

The native Windows app for carbons: WinUI 3 in C# on .NET 10 and the Windows App SDK, with call
audio from `ring_ffi.dll`, a small Rust library over `ring-core` and LiveKit (WASAPI with WebRTC
echo cancellation, noise suppression and gain control). It is unpackaged and self-contained: a
plain folder with `Ring.exe`, distributed as a zip or an MSI, for x64 and arm64 on Windows 10
1809 or later.

Ring sends no telemetry. The .NET CLI's own telemetry is switched off in CI with
`DOTNET_CLI_TELEMETRY_OPTOUT=1`; do the same on your machine.

## Layout

| Path | What it is |
|---|---|
| `Ring/Ring.csproj` | The app (assembly `Ring`, AppUserModelID `TeamOfSilicons.Ring`) |
| `Ring/Program.cs` | Entry point: single instance through `AppInstance`, then XAML |
| `Ring/App.xaml(.cs)` | Theme resources and styles, tray, ringing windows, notification actions |
| `Ring/Themes/Tokens.xaml` | The design tokens of `udd/design.md` as light and dark theme resources |
| `Ring/Api/` | REST client, wire models (`udd/api.md`) and the realtime socket |
| `Ring/Services/` | Session (DPAPI), sign-in (PKCE + loopback), call state machine, sounds, notifications, tray, startup |
| `Ring/Media/` | P/Invoke for `ring_ffi.dll` and the `CallMedia` wrapper |
| `Ring/Views/`, `Ring/Controls/` | Windows, pages and controls |
| `native/ring_ffi.h` | The C contract of `ring_ffi.dll` (`crates/ring-ffi`) |
| `installer/Ring.wxs` | The MSI (WiX Toolset 5) |
| `scripts/probe-ring-ffi.ps1` | Loads `ring_ffi.dll` and checks its C ABI |

Fonts, sounds and the Ring mark come from `assets/` at the repository root and are copied beside
`Ring.exe` at build time.

## Build

You need Windows 10 or 11 (x64 or arm64) with:

- Rust 1.98 with the MSVC target (`rustup target add x86_64-pc-windows-msvc` or `aarch64-pc-windows-msvc`)
- Visual Studio 2022 Build Tools with the C++ workload (for libwebrtc's C++ glue)
- on arm64 only, `clang` on `PATH` (the VS "C++ Clang tools" component or LLVM): the `ring`
  crate under rustls compiles its C with clang for `aarch64-pc-windows-msvc`
- the .NET 10 SDK

From the repository root, in PowerShell:

```powershell
# 1. Call audio. libwebrtc uses the static CRT.
$env:RUSTFLAGS = "-C target-feature=+crt-static"
cargo build --release -p silicon-ring-ffi --target x86_64-pc-windows-msvc
Remove-Item Env:RUSTFLAGS
pwsh windows/scripts/probe-ring-ffi.ps1 -Dll target/x86_64-pc-windows-msvc/release/ring_ffi.dll -Devices

# 2. The app, self-contained, into out/Ring.
dotnet publish windows/Ring/Ring.csproj -c Release -r win-x64 -p:Platform=x64 --self-contained true -o out/Ring
.\out\Ring\Ring.exe
```

For arm64 use `aarch64-pc-windows-msvc`, `-r win-arm64` and `-p:Platform=ARM64`. The project picks
up `target/<triple>/release/ring_ffi.dll` on its own; pass `-p:RingFfiDll=<path>` to use another
build. Without the DLL the app still builds and runs, but calls have no audio.

`ring_ffi.dll` for x64 also cross-builds from macOS or Linux with
[cargo-xwin](https://github.com/rust-cross/cargo-xwin):
`RUSTFLAGS="-C target-feature=+crt-static" cargo xwin build --release -p silicon-ring-ffi --target x86_64-pc-windows-msvc`.
The app itself needs Windows (the XAML compiler is Windows-only).

The MSI, after publishing:

```powershell
dotnet tool install --global wix --version 5.0.2
wix build windows/installer/Ring.wxs -arch x64 -d PublishDir=$PWD\out\Ring -d Version=1.0.0 -o Ring-windows-x64.msi
```

CI does all of this on `windows-2025` (x64) and `windows-11-arm` (arm64): see
`.github/workflows/windows.yml`. It uploads `Ring-windows-<arch>.zip` and `.msi` as artifacts.
Nothing is code-signed yet, so SmartScreen warns on first run.

## Run against a development server

Ring talks to `https://backend.ring.teamofsilicons.com` by default. To use a local `ring-server`:

- click **Server** under the sign-in button and enter, for example, `http://192.168.29.75:8765`; or
- start Ring with `RING_API_URL` set.

A session stays bound to the server that issued it; sign out to switch.

## How it works

- **Sign-in.** `GET /v1/meta` gives the Accounts URL. Ring opens the hosted sign-in page in the
  default browser with PKCE (S256) and a random `state`, listens on
  `http://127.0.0.1:<free port>/auth/callback` with `HttpListener`, then posts the code to
  `POST /v1/auth/carbon`. The `rst_…` token is encrypted with DPAPI for the current user in
  `%LOCALAPPDATA%\Ring\session.dat`.
- **Always listening.** Ring registers itself under `HKCU\…\Run` (`Ring.exe --background`) the
  first time you sign in, keeps a WebSocket to `/v1/ws` open from the tray, pings every 25 s,
  reconnects with backoff and reconciles with `GET /v1/calls?state=ringing,active,voicemail` after
  every reconnect. Closing the window keeps it running while "Keep running in the tray" is on.
- **Ringing.** `call.incoming` opens an always-on-top ringing window (bottom right, without stealing
  focus), shows an `IncomingCall` notification with Answer and Decline, and plays the Ring
  ringtone. Answering or silencing anywhere stops it everywhere (`call.ringing_stopped`).
- **Calls.** The media grant from `POST /v1/calls`, `/accept` or `/media` goes to
  `ring_media_connect`. Events (state, active speakers, levels) come back as JSON on a native
  thread and are marshalled to the UI thread. If audio drops, Ring reclaims the call with
  `POST /v1/calls/{ringid}/media` within the server's 30 s grace period.
- **Handoff.** When the call is active on another of your devices, a banner offers
  "Switch to this PC"; when another device takes it, this one disconnects on `call.handoff`.
- **Data.** Ring keeps `session.dat` (DPAPI) and `preferences.json` (theme, tray, audio devices,
  server) in `%LOCALAPPDATA%\Ring`. Under `HKCU` it writes only the Run entry ("Open at login") and
  the notification registration Windows needs for an unpackaged app. It talks to nothing but the
  Ring server, its LiveKit media server and Silicon Accounts.
