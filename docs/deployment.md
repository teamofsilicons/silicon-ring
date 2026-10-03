# Deploy Silicon Ring

Build and verify from the repository root:

```sh
cargo test --workspace
cargo check -p ring-providers --features s3
npm --prefix web ci
npm --prefix web run build
npm --prefix web test
```

The explicit provider smoke commands make short, paid requests using the keys in `.env`:

```sh
cargo run -p ring-providers --example smoke -- live
cargo run -p ring-providers --example smoke -- stt
```

The Live check requests a fixed phrase in Gleam and accepts the WAV only if the generated transcript matches. The STT check synthesizes a fixed phrase with OpenAI TTS and checks Deepgram's transcription. Neither prints keys or private conversation content.

## Configuration

Keep server credentials in an environment file with mode `0600`, or AWS Secrets Manager. `deploy/server.env.example` lists the fields. Never put server keys in frontend build variables. The native service binds `127.0.0.1:8765`, stores state in `/var/lib/ring`, and serves the web bundle from `/opt/ring/current/web` behind Caddy.

Set `RING_ENV=production` and a base64-encoded 32-byte `RING_ENCRYPTION_KEY`. Back up this key independently: the credential vault cannot recover IAM access/refresh tokens without it. In local development the vault can generate a protected local master key. The SQLite state, credential vault and unfinished recording files must survive restarts. A restarted media session is marked interrupted rather than represented as still connected.

Ring device sessions expire after 30 days so idle native listeners remain available. IAM is rechecked on control operations and every 30 seconds for connected identities and active representatives, including representatives whose CLI session has expired. Logout and authority revocation clear push registrations and pending pushes. Test native push is disabled by default; `RING_TEST_NATIVE_PUSH_ENABLED=true` permits only devices explicitly registered with the sandbox environment in an isolated test server. APNs uses its sandbox endpoint; FCM has no separate sandbox provider endpoint, so only the debug device registration is eligible.

For this setup, protected registration and table-key receipts live in `deploy/*.private.json`; the assembled runtime environment is `deploy/server.env`. These files are excluded from Git. Preserve them privately and never paste their contents into support reports.

## IAM and notifications

`deploy/iam-app.json` is the reviewed app registration request: `ring`, owned by `tos`, backend `https://backend.ring.teamofsilicons.com`, webhook `/webhook/`, scoped identity and directory reads, and Ting send/subscription permissions. `python3 deploy/register-iam.py` registers it and saves the one-time credentials with mode `0600`. It refuses to overwrite an earlier receipt, because a lost reply does not prove registration failed.

The deployed IAM service was verified as version **5.1.0**. The provider crate pins that official SDK. Login exchanges an IAM short-lived token, then introspects the resulting access token and checks actor, audience, selected membership, realm and expiry. Directory reads use the represented user's app token. Webhooks are authenticated over their exact bytes with the SDK and durably deduplicated before HTTP 204. Current IAM authorization remains necessary even when webhook delivery is delayed.

IAM 5.1 uses separately consented OBO access/refresh pairs. Ring exposes consent preparation and code exchange; it must not infer notification authority from an app login. Retain grant refresh operations and immutable notification bodies in encrypted/durable storage. A retry preserves the Ting event key. Ting's live review policy confirms reusable OBO access tokens with exact endpoint, current account, organization and environment checks. Live delivery still needs verification with an approved Ring grant. A failed publication stays visible and retryable according to its actual error.

A production IAM registration can require platform review for critical directory and external scopes. During this setup Ring was created with status `under_review`, and a real login was rejected because the application was not verified. The effective self scopes were present, but directory/Ting scopes were not active. On 2026-10-03 both scope-review requests were submitted with per-scope justification:

- [IAM directory review](https://iam.teamofsilicons.com/scope-reviews?request=7badb5d6-6451-4e58-a7ed-c202401f9658): `7badb5d6-6451-4e58-a7ed-c202401f9658`.
- [Ting endpoint review](https://iam.teamofsilicons.com/scope-reviews?request=e89a03dd-27af-4360-97d4-3960b25e5edc): `e89a03dd-27af-4360-97d4-3960b25e5edc`.

Both remained pending at the last check. No production Ring identity or notification test is valid until that gate is cleared. Webhook activation separately requires the authorized administrator's IAM step-up flow. Do not change scopes or approve a different endpoint merely to avoid review.

Honeycomb and Space Station CLI sessions were refreshed successfully through fresh IAM SLTs. The active Honeycomb actor has organization administration rights in `tos`, but is not a platform validator. Ring is registered directly in IAM and is not yet present in Honeycomb. The supported Honeycomb legacy-adoption workflow requires that IAM first verify the existing application. Use `deploy/honeycomb-catalog.json` for the catalog metadata after approval, retain the existing Ring identity, and follow Honeycomb's operator adoption procedure. Do not run a second identity creation. Honeycomb publication also requires its reviewed application configuration and a valid six-target production CLI release; uploading code alone is not publication approval. No Honeycomb publication request has been submitted yet.

Once Honeycomb recognizes Ring's identity and type-management authority, run `python3 deploy/register-ting-types.py` from an authorized Ting session. The manifest in `deploy/ting-types.json` covers the server's published event types. The Ting session was refreshed, but the current catalog check returned `catalog_authorization_required`; Ting's separate Honeycomb access consent must be completed before type management. A recipient also needs an active Ring subscription: after explicit IAM consent, register it with the `subscriptions.register` token before using the separately scoped `tings.send` token. Test Ting calls require `RING_TING_TEST_APP_SECRET` and `RING_IAM_TEST_ENVIRONMENT_KEY`; production credentials are never a fallback.

## Voice providers

- Live model: `gpt-live-1`, client delegation, server WebSocket at `/v1/live/sessions`, PCM16 mono 24 kHz. Startup waits for `session.started`; shutdown drains `session.closed` to confirm final usage.
- Natural Voices: `ripple`, `vesper`, `willow`, `stone`, `gleam`, `meridian`, `bossa`, `tempo`; default `gleam`.
- Speech endpoint model: `gpt-4o-mini-tts`. Its current voice list differs from Natural Voices. A real Gleam TTS request returned HTTP 400. Ring never silently substitutes another voice.
- Matching Natural Voice messages use a bounded GPT-Live readback instead. They allow 1–400 characters, verify normalized words against the requested text, and fail if they differ. Live continuously emits silence, so the adapter detects audible samples and a two-second quiet tail before closing. This is Live synthesis, not a claim that the separate TTS endpoint supports Gleam.
- Voicemail STT: Deepgram `nova-3`, `language=multi`. The adapter also exposes a PCM24k streaming connection for carbon-call captions. Deepgram is never used for voice generation.

The fixed-phrase Live smoke returned a verified 216,044-byte WAV. The OpenAI TTS → Deepgram multilingual smoke also passed. These checks prove account/model access and the adapter wire paths, not full conversational quality or call capacity.

References: [OpenAI Live WebSockets](https://developers.openai.com/api/docs/guides/voice-websockets), [Natural Voices and session lifecycle](https://developers.openai.com/api/docs/guides/live-conversations), [client delegation](https://developers.openai.com/api/docs/guides/live-delegation), [TTS voices](https://developers.openai.com/api/docs/guides/text-to-speech), [Deepgram models](https://developers.deepgram.com/docs/models-languages-overview).

## Storage and telemetry

Enable the Rust `ring-providers/s3` feature for S3. The official AWS SDK uses the normal credential chain, including an EC2 role. Uploads request server-side AES-256 encryption. An organization's BYO configuration can create an isolated client with its own region, bucket, prefix and credentials. API access control and participation slicing must run before reading or serving a recording; the bucket remains private.

Four ordinary Space Station tables were provisioned in `tos`: `ringbackend`, `ringcli`, `ringwebanalytics`, and `ringwebevents`. Their keys were saved privately. The backend key uses `RING_TELEMETRY_BACKEND_KEY`; the other source keys are `RING_TELEMETRY_CLI_KEY`, `RING_TELEMETRY_WEB_ANALYTICS_KEY`, and `RING_TELEMETRY_WEB_EVENTS_KEY`. Use separate source recorders. The provider accepts only source, step, progress, trace ID and duration; conversation bodies and credentials are excluded. Respect effective local, actor and organization opt-outs.

## Native AWS deployment

The deployed service runs the Rust binary directly under systemd, with Caddy providing HTTPS and WebSocket proxying. No container runtime is needed. On 2026-10-03 both public domains returned successful HTTPS responses and the native service passed its readiness check. The private S3 SDK smoke wrote, read, byte-checked, and deleted its unique test object successfully. The native CloudFormation update completed successfully, and the same EC2 instance passed HTTPS and native-service checks after its restart; both Docker units remain disabled. IAM verification and the notification approvals above remain separate gates.

Build an exact committed revision on an isolated temporary Amazon Linux 2023 CodeBuild worker:

```sh
python3 deploy/build-aws.py --revision COMMIT --bucket PRIVATE_RELEASE_BUCKET
```

This builds Rust with S3 support and the web application using the same operating-system family as the EC2 host. It launches the resulting binary in an isolated test environment before uploading a checksummed archive containing `ring-server` and `web/`. The worker gets only committed build sources, an exact artifact upload permission, and log permissions; it never receives production provider credentials. The script deletes its temporary project, role, source bucket, and log group, and saves a protected receipt in `deploy/aws-build.private.json`. Failed startup probes do not publish a release. Runtime and compiler dependencies are not installed on the production host.

Store the server environment as a Secrets Manager JSON object. Choose a public subnet with an internet gateway, then provision the stack:

```sh
export RING_RELEASE_BUCKET='private-release-bucket'
export RING_RELEASE_KEY='releases/COMMIT/ring-linux-amd64-ID.tar.gz'
export RING_RELEASE_SHA256='64_CHARACTER_SHA256'
export RING_SECRETS_ARN='arn:aws:secretsmanager:REGION:ACCOUNT:secret:NAME'
export RING_VPC_ID='vpc-...'
export RING_SUBNET_ID='subnet-...'
./deploy/deploy-aws.sh
```

`deploy/aws.yaml` provisions an encrypted private S3 bucket, encrypted 30 GiB EC2 disk, instance role, SSM administration, and Elastic IP. There is no SSH ingress. The instance role reads the named runtime secret and release object and operates only on its assets bucket. Caddy obtains certificates after the two DNS A records point to the returned address; preserve unrelated Namecheap records. Ring and Caddy run under separate non-login users, with restricted writable paths and no new privileges.

The archive is verified before extraction. The binary and static files live in `/opt/ring/releases/SHA256/`, with `/opt/ring/current` selecting the active release. Configuration is root-readable at `/etc/ring/server.env`; state remains under `/var/lib/ring`. Signed CLI release metadata is published separately under `/var/lib/ring-public/releases/` and served at `https://ring.teamofsilicons.com/releases/`.

Existing instances do not rerun cloud-init when CloudFormation user data changes. Apply updates explicitly:

```sh
python3 deploy/update-instance.py --instance-id i-... \
  --release-bucket "$RING_RELEASE_BUCKET" --release-key "$RING_RELEASE_KEY" \
  --release-sha256 "$RING_RELEASE_SHA256" --secret-arn "$RING_SECRETS_ARN" \
  --assets-bucket PRIVATE_ASSETS_BUCKET --region us-west-1
```

The updater verifies the archive and launches a short isolated startup probe before switching the release symlink. It restarts the service and requires successful local readiness. A failed update restores the preceding release and environment where one exists. It preserves state and Caddy configuration. Inspect service status and logs through SSM (`systemctl status ring caddy`, `journalctl -u ring`); never paste environment files or credential-bearing logs into reports.

Back up the SQLite state and encrypted credential vault together with the separately protected master key before replacing the instance. This is a single host: a process or instance restart interrupts active media sessions. Interrupted recordings retain their actual coverage state.

## Capacity verification

The default `t3.small` is a candidate until the measured workloads pass. Test the smallest candidate with both the relay workload and actual provider sessions; a byte-relay result alone does not establish Live capacity. Instance type changes cause a maintenance interruption.

For a local optimized baseline:

```sh
cargo build --release -p ring-server --features ring-providers/s3
node scripts/load-test.mjs --binary target/release/ring-server --calls 15 --seconds 30 --report /tmp/ring-load.json
```

This starts an isolated server and thirty test identities, sends 20 ms PCM frames in both directions of fifteen calls for thirty seconds, measures latency and process CPU/RSS, and downloads every finalized recording through the real protocol. Distinct patterns verify that each receiver gets only the other sender and that every input frame survives recording. Missing frames, wrong audio, unfinished calls, or partial coverage cause failure. These fixtures disable external providers and telemetry.

The optimized development-machine baseline at `docs/benchmarks/2026-10-03-local-release.json` passed: 45,000/45,000 frames, fifteen complete recordings without missing intervals, relay p50 30.68 ms / p95 47.87 ms, CPU 13.53% of one core, and peak RSS 19.27 MiB. The earlier failed debug baseline is retained separately; neither is an AWS capacity claim.

On the deployed `t3.small`, the 15-call relay check passed with all 45,000 frames and all recordings complete: p50 25.79 ms, p95 43.64 ms, maximum 111.50 ms, CPU 19.15% of one core, and peak RSS 18.29 MiB. The separate 15-call actual OpenAI Live/Deepgram check also passed: all representatives produced audible PCM within 3.782 seconds, all remained active for the measured thirty seconds, and every recording was complete with no missing intervals. That workload used 24.23% of one core and peaked at 29.00 MiB RSS. See `docs/benchmarks/2026-10-03-aws-t3-small-relay.json` and `docs/benchmarks/2026-10-03-aws-t3-small-live.json`; both identify the deployed archive checksum.

Keep `t3.small` for this workload. T3 baseline capacity totals 10% of one core for nano, 20% for micro, and 40% for small. The measured live workload exceeds micro's baseline even before allowing for operating-system and HTTPS work. The deployed instance currently uses Unlimited credit mode; sustained use above the baseline can incur surplus-credit charges. These thirty-second tests establish bounded concurrency and recording integrity, not a long-running soak or a guarantee for arbitrary speech patterns, failure rates, or network conditions. Monitor CPUCreditBalance, CPUSurplusCreditBalance and CPUSurplusCreditsCharged in operation. See [AWS CPU credit baselines](https://docs.aws.amazon.com/AWSEC2/latest/UserGuide/burstable-credits-baseline-concepts.html) and [Unlimited mode](https://docs.aws.amazon.com/AWSEC2/latest/UserGuide/burstable-performance-instances-unlimited-mode-concepts.html).

Run isolated native workloads on the actual deployed host:

```sh
python3 deploy/benchmark-aws.py --instance-id i-... --bucket PRIVATE_ASSETS_BUCKET \
  --mode relay --report docs/benchmarks/aws-relay.json
python3 deploy/benchmark-aws.py --instance-id i-... --bucket PRIVATE_ASSETS_BUCKET \
  --mode live --secret-arn "$RING_SECRETS_ARN" --report docs/benchmarks/aws-live.json
```

Both helpers use the currently deployed binary in a separate native systemd process, loopback port, protected credentials, and data directory. They read server resource use from Linux `/proc`, save reports in private S3 and locally, then remove their temporary service, fixture objects, and local data. They never use production account tokens. The Live mode makes bounded paid OpenAI and Deepgram calls: fifteen independent representatives must produce audible PCM, remain active for thirty seconds, retain speech transcripts, and finalize complete recordings. Only the two voice-provider keys enter that isolated server; IAM, Ting, and native push are excluded.

For custom remote fixtures, `--prepare-fixtures FILE` on either Node script creates a protected client file and `FILE.tokens.private.json` for the server. Use only a separate test process; never enable fixture credentials in production. `--url wss://HOST/ws --secrets-file FILE` runs against that process, and `--pid PID` measures resources when it is on the same host.

Verify the actual S3 adapter independently:

```sh
RING_S3_BUCKET=PRIVATE_ASSETS_BUCKET AWS_REGION=us-west-1 \
  cargo run -p ring-providers --features s3 --example smoke -- s3
```

Temporary physical-device checks can use `deploy/device-check.py` with a protected fixture file. It creates an isolated native service and `/device-check/` proxy route, and copies only voice/native-push configuration into that service. After the physical checks finish, run `python3 deploy/device-check.py --instance-id i-... --bucket PRIVATE_ASSETS_BUCKET --remove` to remove the temporary service, route, credentials and data. Production IAM remains enforced on the main endpoint.
