# Silicon Ring

Calls, conferences and voicemail for verified carbon (`c:handle`) and silicon (`si:handle`) identities. Ring includes a Rust server, reusable WebSocket client, CLI/audio daemon, SolidJS web client and Tauri native client.

## Native app downloads

Download v0.1.2 for macOS, Windows, and Linux on both arm64 and x86_64, or the Android arm64 APK for Android 8.0+, signed with a stable release key. See [downloads and installation notes](docs/downloads.md) for installers, checksums, the Android certificate fingerprint, and build provenance. The macOS packages use ad-hoc signing and are not notarized; Windows installers are not Authenticode signed. The [web app](https://ring.teamofsilicons.com) is also available.

The [native app guide](native/README.md) covers local desktop/mobile builds and physical-device verification. Development iPhone packages are provisioned for specific devices and are not public release assets.

## Build and check

Use current stable Rust, Node.js 22+ and Python 3. Linux native audio also needs `pkg-config` and ALSA development headers (`libasound2-dev` on Debian/Ubuntu).

```sh
cargo build --workspace
cargo test --workspace
npm --prefix web ci
npm --prefix web test
npm --prefix web run build
python3 crates/ring-cli/tests/smoke.py
node scripts/test-server.mjs
python3 scripts/test-packaging.py
python3 scripts/test-packaging.py --ring-binary target/debug/ring
```

The smoke tests launch a real local server with isolated test identities and provider calls disabled. They check call lifecycle, media protocol, ownership/privacy, persistence, explicit context approval and exact retries. Packaging tests use inert fixtures; `--ring-binary` additionally runs this host's packaged setup with a clean PATH and verifies daemon startup and preserved settings. On Windows, pass `target/debug/ring.exe`. The GitHub native matrix builds and tests all six targets separately.

Provider checks are opt-in and use paid OpenAI/Deepgram credentials from `.env`:

```sh
node --env-file=.env scripts/test-live.mjs --paid-provider-smoke
node --env-file=.env scripts/load-live.mjs --paid-provider-smoke --calls 15 --seconds 30
```

The first checks actual speech, captions and private delegation. The second measures concurrent representative sessions and recording coverage. `scripts/load-test.mjs` separately verifies byte-exact two-way PCM relay at 15 concurrent calls. Saved capacity reports include failures as well as passing runs under [benchmarks](docs/benchmarks).

## Run locally

Build the web client first, then start a development server. This example uses disposable identities and makes no paid provider requests:

```sh
mkdir -p data
umask 077
cat > data/test-tokens.json <<'JSON'
{
  "local-alice-token": {"actor":"c:alice","org_id":"demo","display_name":"Alice"},
  "local-bob-token": {"actor":"c:bob","org_id":"demo","display_name":"Bob"},
  "local-silicon-token": {"actor":"si:assistant","org_id":"demo","display_name":"Assistant"}
}
JSON
RING_ENV=development \
RING_TEST_APP_SECRET=local-only-ring-integration-secret \
RING_TEST_TOKENS_FILE=data/test-tokens.json \
RING_DISABLE_PROVIDERS=1 \
RING_TELEMETRY_ENABLED=false \
cargo run -p ring-server
```

Open [the local app](http://127.0.0.1:8765). In **Connection settings**, select organization `demo`, **Test environment**, and the test secret above. Log in with one of the corresponding local tokens. Use separate browser profiles for Alice and Bob. Browser audio needs microphone permission. Provider-disabled silicons exercise the protocol but do not generate speech.

For the CLI, use a separate terminal:

```sh
export SILICON_HOME="$PWD/data/cli-user"
export SILICON_ORG=demo
export SILICON_RING_SERVER_URL=ws://127.0.0.1:8765/ws
export SILICON_RING_TEST_APP_SECRET=local-only-ring-integration-secret
export PATH="$PWD/target/debug:$PATH"
printf '%s\n' local-silicon-token | ring --test login --token-stdin
ring --test login status --json
ring --test call init c:alice --context 'Discuss the launch.' --start 'Hello!'
ring --test call history
```

Login the recipient first. An accepted init means **ringing**, not answered. Ordinary CLI commands return after the server accepts them; the daemon keeps carbon audio running. `ring call watch RINGID` observes events without owning or ending the call.

## Install and authenticate

From a reviewed checkout, install the native CLI and start its daemon:

```sh
export SILICON_HOME="$HOME/.silicon"
RING_INSTALL_FROM_SOURCE=1 sh scripts/install.sh
export PATH="$SILICON_HOME/.ring/install/bin:$PATH"
```

Install the signed CLI binary and start its daemon with the public installer. Python 3 and OpenSSL 3 with Ed25519 support must be on PATH:

```sh
export SILICON_HOME="${SILICON_HOME:-$HOME/.silicon}"
curl -fsSL 'https://ring.teamofsilicons.com/install.sh' | sh
export PATH="$SILICON_HOME/.ring/install/bin:$PATH"
```

It verifies the pinned Ed25519 key and binary digest before installing. Publication status, IAM review and deployment prerequisites are tracked in [deployment documentation](docs/deployment.md). Installation does not authenticate or request passwords/provider keys. Obtain a short-lived Ring app token from IAM, then:

```sh
ring login --token-stdin
ring login status --json
ring notifications authorize
```

Use the signed installer above or run `honeycomb install ring` for the latest approved Honeycomb package. CLI `0.1.1` and later fix setup before PATH activation and default to the public backend. Existing local `server_url` values and `SILICON_RING_SERVER_URL` overrides take precedence. See [publication status](docs/deployment.md) for release availability.

The last command starts the explicit Ting consent flow for silicon notifications; finish it with `--authorization-id ID --code CODE`. Ting owns notification delivery and interpreter webhooks.

## Context approval

A nonempty stored default is never silently approved:

```sh
ring config set '{"representative.default_context":"Keep replies concise."}'
ring context show --init c:alice --context 'Discuss launch timing.'
ring context approve PREPARATION_ID --for 1h
ring call init c:alice --context 'Discuss launch timing.'
```

Preparing and approving do not place a call. Editing the default invalidates its approvals. Context is limited to 400 Unicode code points, start text to 100, and each representative message to 160. Limits reject excess input without truncation.

## Build on Ring

- [Complete command tree](udd/cli.md), also bundled in `ring --help` and every command's help.
- [WebSocket API and semantics](udd/api.md).
- [CLI storage, daemon, audio, releases and platform validation](docs/cli-runtime.md).
- [Deployment, IAM/Ting, provider integration, S3 and operational status](docs/deployment.md).
- [Original product requirements](udd/understanding.md).
- [`ring-client`](crates/ring-client/src/lib.rs) for reusable async Rust transport.
- [`ring-providers`](crates/ring-providers) for IAM, Live, speech, Ting, S3 and telemetry adapters.

Use the same request ID only for an exact retry. A lost reply does not prove a mutation failed. The CLI prints its base `request_id` and derives stable step IDs for composed operations. `ring doctor` produces a redacted report locally; `ring bug report` submits only the content you explicitly supply.
