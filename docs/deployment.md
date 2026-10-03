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

Keep server credentials in an environment file with mode `0600`, or AWS Secrets Manager. `deploy/server.env.example` lists the fields. Never put server keys in frontend build variables. The image uses `RING_BIND=0.0.0.0:8765`, `RING_DATA_DIR=/data`, and `RING_WEB_DIR=/app/web/dist`.

Set `RING_ENV=production` and a base64-encoded 32-byte `RING_ENCRYPTION_KEY`. Back up this key independently: the credential vault cannot recover IAM access/refresh tokens without it. In local development the vault can generate a protected local master key. The SQLite state, credential vault and unfinished recording files must survive restarts. A restarted media session is marked interrupted rather than represented as still connected.

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

## Docker and AWS

For an existing Linux host with DNS already pointing to it:

```sh
RING_ENV_FILE=./server.env docker compose -f deploy/compose.yaml up -d --build
```

Caddy terminates TLS for both public hosts and proxies HTTP/WebSocket traffic to the Rust service. `/health` is the readiness endpoint. The Docker build copies only manifests, source folders and frontend build output; it never copies `.env` or registration receipts.

For AWS, build and push an image to ECR, capture its immutable SHA-256 digest, and store the runtime environment as a Secrets Manager JSON object. Choose a public subnet with an internet gateway. Then run:

```sh
export RING_IMAGE_URI='ACCOUNT.dkr.ecr.REGION.amazonaws.com/silicon-ring@sha256:DIGEST'
export RING_SECRETS_ARN='arn:aws:secretsmanager:REGION:ACCOUNT:secret:NAME'
export RING_VPC_ID='vpc-...'
export RING_SUBNET_ID='subnet-...'
./deploy/deploy-aws.sh
```

`deploy/aws.yaml` provisions an encrypted private S3 bucket, encrypted 30 GiB EC2 disk, instance role, SSM administration, Docker, Caddy, and an Elastic IP. The host has no SSH ingress. The role can read the named runtime secret and its bucket; ECR pulls and SSM use the corresponding AWS managed policies. Both DNS A records must point to the returned address before Caddy obtains certificates. Preserve unrelated Namecheap records.

The default `t3.small` is a **benchmark candidate**, not a tested capacity promise. Before claiming 10–15 concurrent calls, measure sustained duplex mixing, Live sessions, recording writes, reconnects and notification delivery at that load, including tail latency, CPU and memory. Change the instance type based on results. This is a single host; plan an explicit maintenance interruption for replacement, and back up SQLite/vault data before replacing it.

Run the actual PCM relay and recording benchmark against an optimized server:

```sh
cargo build --release -p ring-server --features ring-providers/s3
node scripts/load-test.mjs --binary target/release/ring-server --calls 15 --seconds 30 --report /tmp/ring-load.json
```

The script starts an isolated server and thirty test identities, sends 20 ms PCM frames in both directions of fifteen calls for thirty seconds, measures relay latency and process CPU/RSS, and downloads every finalized recording through the real protocol. Distinct audio patterns verify that each receiver gets only the other sender and that every input frame survives in the recording. Missing frames, wrong audio, unfinished calls, or partial recording coverage produce a nonzero exit. Automatic local runs disable external voice providers and telemetry; this benchmark does not measure external Live, STT, IAM, Ting or S3 capacity.

For a server already configured with isolated fixture credentials, use `--url wss://HOST/ws --secrets-file FILE`. Generate protected inputs with `--prepare-fixtures FILE`; the companion `FILE.tokens.private.json` is the server's `RING_TEST_TOKENS_FILE`, and `test_app_secret` supplies `RING_TEST_APP_SECRET`. Keep these credentials private and use a separate benchmark process/data directory. `--pid PID` can collect resource usage when the target process is on the same machine. The initial development-machine debug run failed recording completeness, so it is not evidence for the AWS instance's supported capacity.

`aws cloudformation validate-template --template-body file://deploy/aws.yaml` passed. The environment had working AWS account, GitHub and Namecheap access. Docker was installed but its Colima daemon was initially stopped. Those observations do not themselves confirm a deployed service. Check the actual stack, HTTPS endpoints, IAM login, real media, S3 persistence, notification receipt and recovery after deployment.
