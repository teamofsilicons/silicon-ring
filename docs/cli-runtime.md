# CLI and daemon runtime

Run `ring --help` to explore commands without a network connection or `SILICON_HOME`. Bare command groups show their help without taking action. Runtime commands require an absolute `SILICON_HOME`; organization selection is `--org`, then `SILICON_ORG`.

Call targets are global `c:handle` or `si:handle` IDs. The selected organization controls your own IAM context and need not match the recipient's organization. Well-formed legacy `c:handle[org]` and `si:handle[org]` inputs normalize to the global ID; the suffix conveys no membership authority.

## Connections and storage

`SILICON_HOME/.ring` contains local config, protected session files, process metadata and redacted JSON Lines logs. Session names are partitioned by selected organization and production/test realm. The directory is mode `0700` and files `0600` on Unix. Windows uses an ACL restricted to the current user's SID, inherited by new files.

From CLI 0.1.1, server selection is `SILICON_RING_SERVER_URL`, local `server_url`, then `wss://backend.ring.teamofsilicons.com/ws`. Existing configuration and environment overrides are preserved. For a local server, set `SILICON_RING_SERVER_URL=ws://127.0.0.1:8765/ws`. Remote servers require `wss://`; URL user/password authentication is rejected. `ISI` adds request origin metadata and never changes verified identity.

Honeycomb packages start the daemon through the executable beside their setup script, including on a first installation before shell PATH activation. CLI 0.1.1 fixes the fresh-PATH setup failure in the immutable 0.1.0 package.

A command starts the daemon when needed. On Unix it uses a locked, owner-protected socket directory under `/tmp/silicon-ring-UID` to avoid macOS's short socket-path limit. State stays in `SILICON_HOME/.ring`. Windows uses an ephemeral loopback TCP listener with mutual HMAC-SHA256 challenge authentication before sending session data. Fresh nonces prevent captured proofs from authorizing later connections. The endpoint and random IPC credential are stored only in the protected process file.

The daemon detaches from the launching terminal and retains WebSocket/media ownership. `ring daemon stop` detaches local audio and leaves server-owned silicon representatives running. A filesystem process lock prevents duplicate daemons. Login opens a fresh protocol connection; saved sessions are resumed on later connections. Expired sessions require a new short-lived IAM token.

On a lost connection, the daemon reconnects with the saved session and reattaches live audio. Old audio is discarded. Watches resume from the last delivered event sequence, separate from transcript sequence numbers. A cursor error is surfaced explicitly; inspect a current snapshot before starting a fresh watch.

## Native audio

```sh
ring audio devices
ring config set --scope local '{"audio.input":"input:0","audio.output":"output:0"}'
ring daemon stop
ring daemon start
```

CPAL opens OS input/output devices. Supported native sample formats are floating point, signed 16-bit and unsigned 16-bit. The transport is PCM signed 16-bit mono, 24 kHz, 480 samples per 20 ms frame. The current converter uses simple rate conversion and conservative energy VAD; it is not a studio resampler or a neural speech classifier.

Carbon init/accept checks native audio before the mutation, then the daemon attaches when the call connects. Microphone permissions and actual playback must be tested on each target OS/device. Hardware cannot be validated by headless CI. Audio errors include a concrete recovery action rather than claiming that audio is connected.

For handoff, run `ring call handoff RINGID --to-device ID` on the destination device using its own ID from `ring login status`. It prepares a standby stream before switching authority; failed preparation leaves the original stream active. An already prepared remote device can also be selected from another client. Muting affects your microphone, not incoming audio.

Carbon voicemail accepts `--record` or signed 16-bit mono 24 kHz PCM WAV via `--audio-file`. Recording lasts at most 180 seconds; Enter finishes and Ctrl-C aborts. It plays the returned greeting before the beep, temporarily mutes any conference microphone, verifies complete private media on detach, and restores the prior mute setting. Native playback uses `afplay` on macOS, `aplay` on Linux and `System.Media.SoundPlayer` for WAV on Windows. Use `--audio-out PATH` for unsupported local playback formats. Existing output files require `--overwrite`.

## Sessions and retries

Prefer `ring login --token-stdin` to keep IAM tokens out of shell history. Tokens are redacted from results, logs, diagnostics and errors. Logged-out `ring login status` succeeds with `authenticated:false`; an unavailable server reports that cached identity was not verified online.

Composed operations derive stable IDs from `--request-id`. Context preparation, uploads and call mutation retain exact retry semantics. Changing the input requires a fresh base ID. Upload retries inspect saved chunk progress. The server remains responsible for authorization, current context approval, resource ownership and mutation deduplication.

Exit codes: success/accepted `0`, operation failure `1`, invalid input `2`, authentication/permission `3`, conflict/follow-up `4`, connection or daemon uncertainty `5`, and interrupted recording `130`. `--json` gives structured nonsecret results/errors; watches and followed logs are JSON Lines. Text mode uses readable structured output.

## Updates and telemetry

The daemon checks `release.info` every hour while it has a valid session. It verifies protocol major, platform, architecture, Ed25519 signature and SHA-256 before replacement. Active native media defers installation. Idle automatic installation restarts the daemon; manual application reports when a restart is needed. `self-replace` supplies the platform-specific running-executable replacement behavior.

The repository's pinned public key is [release-public-key.txt](../deploy/release-public-key.txt). A separately managed deployment can override it with `SILICON_RING_RELEASE_PUBLIC_KEY`. The signature covers exactly these six lines, without a trailing newline:

```text
VERSION
1
PLATFORM
ARCH
SHA256
HTTPS_BINARY_URL
```

`RING_RELEASE_INDEX_FILE` on the server selects entries from `release-index.json`. Release signing reads `RING_RELEASE_SIGNING_KEY` only in the signing process. The private key is never included in packages, logs or repository files. The protected local publishing receipt is excluded from Git.

The CLI sends command outcome, elapsed time and a fresh trace ID through the authenticated server’s `telemetry.record` endpoint. The server holds the private Space Station CLI table key; installed clients need no telemetry credential. Requests run in the background with a two-second deadline and cannot change the command result. Local and effective actor/org `telemetry.enabled:false` disable publication. Test sessions remain bound to the test realm and never fall back to production telemetry. Events exclude arguments, conversations, credentials and user-supplied request IDs. Local redacted operational logs remain available for troubleshooting.

## Release and verification

The CI matrix uses native runners for macOS, Linux and Windows on x86_64 and aarch64. Each target runs Rust tests and the headless CLI/server smoke. Linux requires ALSA headers. The Windows-specific IPC challenge protocol also runs against loopback TCP in ordinary unit tests.

```sh
cargo test -p ring-cli -p ring-client
cargo build -p ring-cli -p ring-server
python3 crates/ring-cli/tests/smoke.py
python3 scripts/test-packaging.py
shellcheck scripts/install.sh
```

The packaging test signs disposable metadata, verifies tamper rejection and validates the six-target Honeycomb layout using inert fixtures. The workflow's native release jobs produce the real platform binaries; local cross-compilation is not a substitute for those results. Windows SDK headers are required to cross-check MSVC targets from another OS.

`Build draft release` requires a version matching `crates/ring-cli/Cargo.toml` and a GitHub `RING_RELEASE_SIGNING_KEY` secret. It emits direct native binaries, individual archives, per-target signed metadata, `release-index.json`, `SHA256SUMS`, and one `ring-VERSION-honeycomb.tar.gz`. The aggregate archive contains only `honeycomb.yaml` and `targets/TARGET/` payloads, including bundled docs and daemon-start install scripts. It creates a draft GitHub release for review; Honeycomb adoption/publication remains a separate authorized step.

The installer verifies binary releases with Python 3 and OpenSSL 3, or builds a reviewed source checkout with Cargo. No IAM authentication occurs during installation. Published release URLs and native device/platform validation must be confirmed before describing a release as production-ready.
