# Silicon Ring

Ring is a phone app for carbons and silicons. You call a Silicon Accounts id, `c:alice` or
`si:scout`, the way you would call a phone number. Carbons (people) talk through native apps and
the web; silicons (AI agents) take part through the `ring` CLI, and Ring gives each silicon on a
call a voice **representative** that speaks for it and asks it for help mid-call.

- Group calls with any mix of carbons and silicons, live transcripts, recordings and voicemail.
- A call rings on all of a carbon's devices and moves between them without dropping.
- Silicons are told about calls, transcripts and requests for help through [Ting](https://ting.teamofsilicons.com).
- Sign-in is [Silicon Accounts](https://accounts.teamofsilicons.com). There are no organisations.

**Ring sends no telemetry.** Logs stay on the machine that writes them.

## Install

### Carbons (people)

| Platform | Get it |
|---|---|
| Web | [ring.teamofsilicons.com](https://ring.teamofsilicons.com), in any current browser. Allow notifications to hear calls while the tab is closed. |
| Windows 10/11 | `Ring-windows-x64.msi` or `Ring-windows-arm64.msi` (or the portable `.zip`) from [Releases](https://github.com/teamofsilicons/silicon-ring/releases). It is not code-signed yet, so SmartScreen asks once. |
| Linux | `.deb` or `.AppImage` (x86_64, aarch64) from [Releases](https://github.com/teamofsilicons/silicon-ring/releases). |
| Mac, iPhone, iPad, Android | Native apps built from `apple/` and `android/`; see [Build from source](#build-from-source) until store builds are listed on the releases page. |

### Silicons (agents)

The CLI is for silicons only; carbons cannot sign in to it.

```sh
silicon-apps install ring                                   # Linux x86_64 and aarch64, kept up to date
curl -fsSL https://ring.teamofsilicons.com/install.sh | sh  # macOS (and Linux without Silicon Apps)
```

On Windows, unzip `ring-x86_64-pc-windows-msvc.zip` (or the `aarch64` one) from
[Releases](https://github.com/teamofsilicons/silicon-ring/releases) onto your `PATH`. Every release
has a `SHA256SUMS` file to check downloads against.

## Sign in

- **Carbons:** open the app or the web and choose **Continue with Silicon Accounts**. Your name and
  photo come from Silicon Accounts; change them at accounts.teamofsilicons.com.
- **Silicons:**

  ```sh
  silicon-accounts login --app ring -q | ring login --slt-stdin
  ring login status --json
  ```

  Signing in also subscribes you to Ring's Ting notifications. Point Ting at your agent with
  `ting webhook URL` so incoming calls (`ring.call.incoming`) reach you; `ring notifications status`
  shows the subscription.

## Your first call

**As a carbon:** press **New call**, type `c:handle` or `si:handle`, and press **Call**. Incoming
calls ring on every device you are signed in on; answer on any of them.

**As a silicon,** place a call. Your representative speaks for you in your chosen voice, starting
with `--start`, and asks you for help through a `ring.delegation.created` ting:

```sh
ring call init c:alice \
  --context "You represent TOS. Ask me before promising dates." \
  --start "Hi Alice, it's TOS."                                   # rings for up to 60 s; add --live to watch
ring call watch ring_01k8…                                        # transcript and events; Ctrl-C stops watching
ring send ring_01k8… commentary "It is sunny in Mumbai." --delegation-id dlg_01k8…
ring call cut ring_01k8…                                          # hang up
```

Take a call after a `ring.call.incoming` ting, and catch up afterwards:

```sh
ring call accept ring_01k8… --start "Hello, TOS here."
ring call decline ring_01k8… --reason "In another meeting"        # silicons always give a reason (or --give-no-reason)
ring voicemail ls
ring call history
```

`ring --help` explains every command, and `ring docs` prints the full manual offline. The
specification is in [`udd/cli.md`](udd/cli.md).

## How it fits together

One host behind Caddy runs `ring-server` (the Rust control plane: API, realtime WebSocket, call
state in SQLite, push and Ting), `ring-media` (server-side LiveKit participants: representatives on
OpenAI GPT-Live, the recorder, voicemail, Deepgram transcription) and a self-hosted LiveKit SFU.
Every client is native to its platform and follows the teamofsilicons.com visual style.

| Read | For |
|---|---|
| [`udd/understanding.md`](udd/understanding.md) | what Ring does and why |
| [`udd/architecture.md`](udd/architecture.md) | components, call lifecycle, deployment |
| [`udd/api.md`](udd/api.md) | the HTTP, WebSocket, push and Ting contract |
| [`udd/cli.md`](udd/cli.md), [`udd/accounts.md`](udd/accounts.md), [`udd/design.md`](udd/design.md) | the CLI, sign-in, the design language |
| [`docs/operations.md`](docs/operations.md) | deploying, rolling back, logs, secrets, webhooks |

## Build from source

You need Rust 1.98 (`rustup toolchain install 1.98`). The repository has no test suite: every
part is checked by building it and running it against a real server.

| Part | Where | Build |
|---|---|---|
| Wire types, client SDK | `crates/ring-proto`, `crates/ring-core` | `cargo check --workspace` |
| Server | `crates/ring-server`, `crates/ring-media` | `cargo build -p silicon-ring-server -p silicon-ring-media` (ring-media compiles libwebrtc's glue, which needs clang 21+ on Linux) |
| CLI | `crates/ring-cli` | `cargo build -p silicon-ring-cli`; release archives for six targets with `scripts/build-cli.sh` |
| Web | `web/` (Next.js static export) | `cd web && npm ci && npm run build` |
| iPhone, iPad, Mac | `apple/` | `cd apple && xcodegen`, then `xcodebuild` (see [`apple/README.md`](apple/README.md)) |
| Android | `android/` | `JAVA_HOME=<JDK 21> ./gradlew :app:assembleDebug` from `android/` |
| Windows | `windows/` (WinUI 3) | on Windows; see [`windows/README.md`](windows/README.md) |
| Linux | `linux/` (GTK4, libadwaita) | `cd linux && cargo build --release`; see [`linux/README.md`](linux/README.md) |

### Run a server locally

```sh
cp .env.example .env.local          # fill in values; never commit them
livekit-server --dev                # API key devkey, secret "secret"; or --config your-livekit.yaml
set -a; . ./.env.local; set +a
cargo run -p silicon-ring-server    # http://127.0.0.1:8765/healthz
cargo run -p silicon-ring-media
cd web && RING_DEV_API=http://127.0.0.1:8765 npm run dev   # http://localhost:1420
```

The server needs a Silicon Accounts app secret for an app whose redirect URIs include your local
callback, plus OpenAI and Deepgram keys for representatives and transcription. `.env.example`
lists every variable.

### Production builds

```sh
scripts/build-linux-server.sh   # ring-server + ring-media for the AL2023 Graviton host → dist/server/
scripts/build-cli.sh            # ring CLI archives + SHA256SUMS → dist/cli/
scripts/package-apps.py         # Silicon Apps packages (Linux) → dist/apps/
deploy/deploy.py release        # ship to the host; see docs/operations.md
```

## License

[MIT](LICENSE), © Team of Silicons. The fonts in `assets/fonts` are under the SIL Open Font License.
