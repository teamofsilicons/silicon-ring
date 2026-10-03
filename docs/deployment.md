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

Calling uses global `c:handle` and `si:handle` IDs. Users from any IAM organization can log in and call a registered Ring user in another organization without a shared organization or a target-organization argument. Ring verifies the recipient through that recipient's retained IAM grant in the same realm and pins each participant's organization context to the call. Voice settings, provider credentials and Ting grants remain in their owner's organization context; greeting generation uses the saved recipient context, and voicemail generation/storage uses the sender's saved context. Production and test identities remain isolated.

IAM 5.1 uses separately consented OBO access/refresh pairs. Ring exposes consent preparation and code exchange; it must not infer notification authority from an app login. Retain grant refresh operations and immutable notification bodies in encrypted/durable storage. A retry preserves the Ting event key. Ting's live review policy confirms reusable OBO access tokens with exact endpoint, current account, organization and environment checks. Live delivery still needs verification with an approved Ring grant. A failed publication stays visible and retryable according to its actual error.

A production IAM registration can require platform review for critical directory and external scopes. During this setup Ring was created with status `under_review`, and a real login was rejected because the application was not verified. The effective self scopes were present, but directory/Ting scopes were not active. On 2026-10-03 both scope-review requests were submitted with per-scope justification:

- [IAM directory review](https://iam.teamofsilicons.com/scope-reviews?request=7badb5d6-6451-4e58-a7ed-c202401f9658): `7badb5d6-6451-4e58-a7ed-c202401f9658`.
- [Ting endpoint review](https://iam.teamofsilicons.com/scope-reviews?request=e89a03dd-27af-4360-97d4-3960b25e5edc): `e89a03dd-27af-4360-97d4-3960b25e5edc`.

At 17:53 UTC on 2026-10-03, IAM reported Ring as `verified` with all four directory scopes and both Ting endpoints approved. A fresh production CLI login, authenticated status, own-profile read, device list, app information and signed release lookup then passed. The two legacy IAM discussion records still reported `pending`; their migrated Honeycomb publication record and IAM's effective scopes confirm activation. The webhook remained `pending_review` with no active URL at that check. Its activation separately requires the authorized administrator's IAM step-up flow for the existing endpoint.

Honeycomb recognized the existing Ring identity and migrated its review discussions into publication request `9d3bc31a-8417-531c-8643-01f6e0ccc31f`, revision 1. The Honeycomb, IAM and Ting review gates were already approved. The existing authorizing manager session was renewed, the validated six-target `0.1.0` production archive was uploaded, and the approved request was activated. Read-back confirmed Ring `active` and `public`, effective revision 1, with the release public and no remaining permission-approval requirement. No duplicate application or platform-review decision was created. Inspect the [public Ring catalog record](https://honeycomb.teamofsilicons.com/api/v1/apps/ring), or use **Sent requests** in the [Honeycomb console](https://console.honeycomb.teamofsilicons.com) and `honeycomb publication get ring`. Protected publication receipts remain in `deploy/honeycomb-publication.private.json`.

[Ring v0.1.1](https://github.com/teamofsilicons/silicon-ring/releases/tag/v0.1.1) was published at 19:16 UTC on 2026-10-03. All 37 downloads passed anonymous HTTPS size and SHA-256 verification. The live signed installer also passed in disposable fresh and preconfigured homes: signature and binary verification, daemon startup, production app information, preserved settings, and complete temporary-daemon cleanup. The live native backend and web app were updated successfully; readiness, HTTPS, protocol negotiation, public app information and the signed six-target update index passed. A fresh authorized production IAM login also passed status, own-profile, device-list and signed release-lookup checks, followed by logout and temporary credential/daemon cleanup. The release includes global-ID calls across organizations, corrected Honeycomb setup and production defaults, and a Windows daemon handle-isolation fix. [All seven CI jobs passed](https://github.com/teamofsilicons/silicon-ring/actions/runs/37146814679), including captured PowerShell output reaching EOF while the daemon remains alive on both Windows architectures. CLI packages come from `db4d331`; unchanged backend/native-app payloads come from `bb076a5`, with native provenance included in the release.

Updating only the Honeycomb catalog description created revision 2 and publication request `5e1b86b9-db19-4a55-ae8a-ab1091db6a06`, without new IAM/Ting scope requests. Its validator subsequently approved the revision, and anonymous catalog reads now confirm Ring is active and public with effective revision 2 and latest version `0.1.2`. The archive's SHA-256 is `68ed6f4ee6c51ec1d0ba7dc4286c6d394441887370598dd4322ede7d1ae57b2a`. The current release needs no further catalog approval.

[Ring v0.1.2](https://github.com/teamofsilicons/silicon-ring/releases/tag/v0.1.2) was published at 20:22 UTC on 2026-10-03 from `baaee314`. All 37 public downloads passed anonymous HTTPS size and SHA-256 verification. The backend and web deployment passed readiness, and a fresh production IAM login passed authenticated status, own-profile, device-list, app-capability and signed release-lookup checks. The public signed index selects `0.1.2`; the HTTPS installer passed in fresh and preconfigured disposable homes, preserving existing configuration and starting its daemon. Temporary credentials and daemons were cleaned up.

Version 0.1.2 fixes private voicemail setup and cleanup across organization contexts, complete transcript pagination with later revisions, S3 asset location preservation, expired draft cleanup, interrupted Ting consent refresh, and hourly signed CLI updates without a login. [All seven CI jobs passed](https://github.com/teamofsilicons/silicon-ring/actions/runs/37150722062), as did [all eight native app jobs](https://github.com/teamofsilicons/silicon-ring/actions/runs/37150722084). Local validation passed 46 Rust tests, 16 real-server protocol checks, seven web tests and four private-recording scenarios. Browser verification loaded 451 transcript entries and retained history while applying a revision and a new entry. A development-signed 0.1.2 iPhone build was installed and launched. At 20:49 UTC its production IAM sign-in succeeded after replacing the old fixture connection settings with the production WebSocket endpoint, selecting the user's own organization and clearing the test secret. The phone showed Connected, the correct account profile and its registered device. This verifies production phone authentication and registration; it is not an additional production call or push-delivery test.

The official `honeycomb install ring --json` path also passed in fresh and preconfigured disposable homes with a minimal PATH. Both setup scripts succeeded, installed the exact public 0.1.2 archive and signed binary, started the daemon, preserved prior configuration byte-for-byte, and reported no update through the pinned-signature check. The fresh installation reached production app information without login. Both temporary daemons were stopped and uninstalled, and their homes were removed; the existing global installation was unchanged.

Ting's separate catalog consent was completed for `honeycomb.apps.list`, bound to the existing manager and `tos`, with only the displayed identity and membership disclosures. All 22 types in `deploy/ting-types.json` were registered and their names and descriptions matched read-back at 18:04 UTC. Ting requires `app.service.event`; the internal `call.invitation.busy` WebSocket event therefore publishes as `ring.call.invitation_busy`, retaining its notification key and payload. `python3 deploy/register-ting-types.py` provisions this manifest for an authorized deployment. A recipient still needs its own explicit IAM consent and active Ring subscription: register with the `subscriptions.register` token before using the separately scoped `tings.send` token. Production Ting delivery has not yet been verified with a real Silicon recipient.

A real-provider test was attempted in a dedicated Honeycomb shared environment, `05e6ad71-de4b-4c92-885b-11e7bf3517d7`. IAM, Honeycomb, Ting and Briefcase confirmed their imports, but Ring failed because Honeycomb has no protected lifecycle transport configured for it. Import operation `9425a1ad-2466-4268-9706-12701a4f59e5` remains pending at revision 2; coordinated clean returned HTTP 409 because the platform requires the pending operation to finish first. No test actors, calls or notifications were created.

Recovery requires a deployed lifecycle receiver and matching Honeycomb participant transport registration, followed by retry of the original import and coordinated clean/delete. The published 0.1.2 runtime has only the configured legacy test realm. The source implementation below adds named environments; its deployment and live provider verification must be recorded separately. Legacy test Ting calls use `RING_TING_TEST_APP_SECRET` and `RING_IAM_TEST_ENVIRONMENT_KEY`. Named environments obtain Ting's own secret and root key from each verified IAM endpoint grant; production credentials are never a fallback.

## Shared Honeycomb test environments

Provision a dedicated service bearer of at least 32 visible ASCII characters in Ring's secret store as `RING_HONEYCOMB_CONTROL_TOKEN`. The Honeycomb operator must provision that same value under its chosen `token_env` and add this participant to `HONEYCOMB_LIFECYCLE_PARTICIPANTS`:

```json
{"app_id":"ring","base_url":"https://backend.ring.teamofsilicons.com","token_env":"RING_HONEYCOMB_CONTROL_TOKEN"}
```

The Honeycomb management adapter also requires its existing `IAM_HONEYCOMB_SERVICE_CREDENTIAL` configuration. Reload each service through its normal deployment process. The root testing key, Ring application secret and user token cannot authorize this transport. Catalog publication and IAM webhook approval are independent of participant registration.

Honeycomb sends authenticated `PUT /internal/honeycomb/organizations/{org}/testing-environments/{uuid}/operations/{operation_uuid}` requests. Ring checks exact path/body identities, application ID, positive revision/generation/key fences and pinned import provenance. Revisions may skip numbers because unrelated app imports need not notify Ring. SQLite retains encrypted environment metadata and durable request fingerprints/receipts outside the environment's generation directory. Identical completed retries replay the saved receipt; changed payloads under the same operation ID conflict.

Prepare/import durably establish the isolated runtime without waiting for IAM's later finalization. The first client hello validates its supplied Ring test secret through IAM, caches it encrypted, and starts workers only after the environment is active. Production, legacy test and each named UUID use separate state. Named scopes disable production telemetry and use a distinct S3 namespace while retaining original bucket/key/credential-source records for cleanup.

Clean and purge first close admission, disconnect sockets and drain workers, including external writes already in flight. Cleanup deletes only that environment's pinned S3 audio/transcript objects before removing local state and credentials. Failed remote cleanup preserves retryable metadata and credentials; it never returns a completed receipt. Disable retains recoverable conversation data while removing session/grant authority. Restore and key rotation require fresh authentication; interrupted calls are ended. Ring retirement disables its runtime; Ting-only retirement drains sends and removes Ting grants, pending consent, queued publications and notification retry receipts while preserving Ring calls and sessions. Fresh notification authorization goes through IAM after Ting becomes available again. Purged environments retain a nonsecret tombstone and cannot be resurrected.

After Honeycomb reports the import ready, connect using credentials issued inside that same environment:

```sh
export SILICON_RING_TEST_APP_SECRET_FILE=/secure/ring-test-app-secret
ring --testing-environment ENVIRONMENT_UUID --org ACTOR_ORG login --token-stdin
ring --testing-environment ENVIRONMENT_UUID --org ACTOR_ORG login status --json
```

The secret file must be protected from other users. In the web/native app choose **Honeycomb test environment**, enter the UUID and Ring test secret, and select the actor's own IAM organization. Each client requires the exact UUID acknowledgment before login/resume. Coordinate cleanup through Honeycomb so every participating service observes the same generation; do not call the internal receiver as a substitute for that workflow.

Select and install a named test environment's release through Honeycomb. Its `release.info` response reports that package management is owned by Honeycomb and offers no automatic Ring update; it never reads the production release index. Production and configured legacy test update behavior is unchanged.

Contract references: [Honeycomb participant transport](https://github.com/teamofsilicons/silicon-honeycomb/blob/89fcca926b640887470d1e4cf9262b27f7eeaaf4/crates/server/src/participant_management.rs), [coordinated lifecycle](https://github.com/teamofsilicons/silicon-honeycomb/blob/89fcca926b640887470d1e4cf9262b27f7eeaaf4/crates/server/src/lifecycle.rs), and the official IAM SDK pinned by this workspace.

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

From 0.1.2, each uploaded asset pins its bucket, region, full object key and managed-versus-organization credential source before the first upload. Later reads, retries and retention deletes use that location and the current credentials; changing the destination does not redirect existing audio. No credential snapshot is stored with the asset. Legacy records containing only a relative key use the current configuration until a successful read or size check verifies their location. If the old destination has already changed, restore access to it before migrating or deleting those legacy recordings.

Four ordinary Space Station tables were provisioned in `tos`: `ringbackend`, `ringcli`, `ringwebanalytics`, and `ringwebevents`. Their keys were saved privately. The backend key uses `RING_TELEMETRY_BACKEND_KEY`; the other source keys are `RING_TELEMETRY_CLI_KEY`, `RING_TELEMETRY_WEB_ANALYTICS_KEY`, and `RING_TELEMETRY_WEB_EVENTS_KEY`. Use separate source recorders. The provider accepts only source, step, progress, trace ID and duration; conversation bodies and credentials are excluded. Respect effective local, actor and organization opt-outs.

## Native AWS deployment

The deployed service runs the Rust binary directly under systemd, with Caddy providing HTTPS and WebSocket proxying. No container runtime is needed. On 2026-10-03 both public domains returned successful HTTPS responses and the native service passed its readiness check. The private S3 SDK smoke wrote, read, byte-checked, and deleted its unique test object successfully. The native CloudFormation update completed successfully, and the same EC2 instance passed HTTPS and native-service checks after its restart; both Docker units remain disabled. IAM verification and Honeycomb publication subsequently completed; webhook activation and real-recipient Ting delivery remain separate checks described above.

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
