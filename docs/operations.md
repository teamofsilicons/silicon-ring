# Operating Silicon Ring

This is the runbook for the production service: one Graviton host in us-west-1 that serves
`ring.teamofsilicons.com` (web app) and `backend.ring.teamofsilicons.com` (API and media). Every
command runs from the repository root on an operator machine with AWS credentials for the account
that owns the `silicon-ring` CloudFormation stack. Nothing here opens SSH: the host is administered
through SSM only.

Ring sends no telemetry. Logs stay in the host's journal (capped at 512 MB) and never contain
tokens or conversation text.

## What runs where

| Piece | Where | Notes |
|---|---|---|
| Stack `silicon-ring` | `deploy/stack.yaml` | EC2 t4g.medium (AL2023 arm64, IMDSv2, encrypted gp3 30 GiB), Elastic IP 13.57.4.48, S3 bucket `silicon-ring-assets-smdjuu0paizr`, the instance role, the security group |
| Runtime secret | Secrets Manager `silicon-ring/production` | One JSON object of environment variables, written by `deploy/secrets.py` |
| `caddy.service` | `/etc/caddy/Caddyfile` (`deploy/Caddyfile`) | TLS on 80/443 (and HTTP/3 on udp/443), the static web export, `/v1/*` → ring-server, `/rtc*` → LiveKit, `/internal/*` blocked; admin API only on `/var/lib/caddy/admin.sock` (caddy user) |
| `ring-server.service` | 127.0.0.1:8765, `/etc/ring/server.env` | Control plane; SQLite at `/var/lib/ring/ring.sqlite3` |
| `ring-media.service` | no listener, `/etc/ring/media.env` | Representatives, recorder, voicemail, Deepgram STT; working files in `/var/lib/ring/media` |
| `livekit.service` | 127.0.0.1:7880, `/etc/livekit/livekit.yaml` (`deploy/livekit.yaml.tmpl`) | ICE 7881/tcp and 7882–7892/udp, TURN 3478/udp |
| `ring-backup.timer` | daily 03:30 UTC | SQLite online backup to `s3://…/backups/` |

Host layout:

```
/opt/ring/releases/<id>/{bin,web,deploy,RELEASE.json}   one directory per release
/opt/ring/current, /opt/ring/previous                   symlinks the units run from
/opt/ring/tools/{caddy-2.11.7,livekit-1.13.10}/         pinned upstream binaries (sha256-checked)
/etc/ring/{server,media}.env                            0600 root, rendered from the secret
/var/lib/ring                                           state (owned by ring, 0700)
/var/backups/ring                                       pre-deploy snapshots and local backups
```

The S3 bucket holds `releases/` (release archives and the installer), `backups/`, and the
application's objects under `production/` (recordings, voicemail).

## Tools you need

- AWS CLI v2 with credentials for the account, region `us-west-1`.
- Python 3.9 or newer.
- For builds: Rust 1.98, colima (an arm64 docker host), Node 24+ for the web app, and
  `cargo-zigbuild` and `cargo-xwin` for the CLI.
- For platform steps: `silicon-apps`, `silicon-accounts` and `ting`, signed in as si:tos.

## Build a release

```sh
colima start                          # if it is not running
scripts/build-linux-server.sh         # ring-server + ring-media for aarch64 AL2023 → dist/server/
(cd web && npm ci && npm run build)   # static export → web/out/
deploy/deploy.py package              # → dist/release/<id>.tar.gz and <id>.json
```

On a slow or flaky network, the container's libwebrtc download can time out. Fetch
`webrtc-linux-arm64-release.zip` (the URL is in the error) on the host, unzip it under `target/`,
and run `LK_CUSTOM_WEBRTC=target/…/linux-arm64-release scripts/build-linux-server.sh`.

The server binaries can also come from the **Server binaries** workflow
(`.github/workflows/media.yml`): download its `ring-server-linux-aarch64` artifact into
`dist/server/`.

A release id is `<version>-<git sha>-<content hash>`, so rebuilding the same tree produces the same
id. `deploy.py package` warns when shipped paths have uncommitted changes.

## Deploy

```sh
deploy/deploy.py release --dry-run    # package and resolve the target; uploads nothing
deploy/deploy.py release              # upload, install over SSM, print the result
```

`release` uploads the archive to `s3://silicon-ring-assets-smdjuu0paizr/releases/<id>.tar.gz`
(skipped when the same bytes are already there), stages `deploy/install-release.sh` next to it, and
runs it as root through `AWS-RunShellScript` on the instance named by the stack's `InstanceId`
output. On the host the installer:

1. takes the deploy lock (`flock /run/ring-deploy.lock`);
2. downloads the archive and checks its SHA-256, then extracts it into `/opt/ring/releases/<id>`
   (refusing links, absolute paths and unexpected top-level entries);
3. installs the pinned Caddy and LiveKit binaries once, each checked against its SHA-256;
4. renders `/etc/ring/server.env`, `/etc/ring/media.env` and `/etc/livekit/livekit.yaml` from the
   secret (key names are printed, values never), and installs the Caddyfile and units;
5. validates the Caddyfile, snapshots the SQLite database to `/var/backups/ring/pre-<id>-<time>`,
   and switches `/opt/ring/current` atomically;
6. restarts in dependency order: LiveKit (only when its binary or configuration changed, because a
   restart drops calls in progress), ring-server (until `/healthz` answers), ring-media (until it
   stays up without restarting), then Caddy (reloaded when only the Caddyfile changed);
7. checks `https://backend.ring.teamofsilicons.com/healthz`, `https://ring.teamofsilicons.com/` and
   `/v1/meta` against 127.0.0.1. A fresh host gets its certificates here, which can take a minute.

If any step fails, the installer puts back the previous release, configuration and database
snapshot, restarts the services and exits non-zero; `deploy.py` prints the host's output and exits 1.
It keeps the last five releases and ten snapshots.

## Roll back

```sh
deploy/deploy.py rollback                    # to /opt/ring/previous
deploy/deploy.py rollback --to <release id>  # to any release still on the host
deploy/deploy.py rollback --restore-db       # also restore the snapshot taken before the current release
```

Without `--restore-db` the database stays as it is, which is right unless the release being rolled
back changed the schema in a way the older one cannot read.

## Status and logs

```sh
deploy/deploy.py status                                   # releases, units, health checks
deploy/deploy.py logs --unit ring-server --lines 300
deploy/deploy.py logs --unit ring-media --since '1 hour ago'
deploy/deploy.py logs --unit livekit
deploy/deploy.py logs --unit caddy
```

For an interactive shell, use Session Manager (needs the `session-manager-plugin`):

```sh
aws ssm start-session --region us-west-1 --target "$(aws cloudformation describe-stacks --region us-west-1 \
  --stack-name silicon-ring --query "Stacks[0].Outputs[?OutputKey=='InstanceId'].OutputValue" --output text)"
sudo journalctl -u ring-server -f
```

`RUST_LOG` in the secret sets the log filter for both Rust services (default `info`).

## Backups and restore

- `ring-backup.timer` copies every SQLite database with SQLite's online backup, checks the copy's
  integrity, and uploads `backups/scheduled-<host>-<UTC>.tar.gz` (SSE-S3). The host keeps the last
  seven archives too.
- `deploy/deploy.py backup --label <label>` does the same on demand. It works on any host with
  Python 3 and the AWS CLI, including pre-1.0 hosts.
- To restore onto a host, for example after the instance was replaced:
  `deploy/deploy.py release --restore-backup s3://silicon-ring-assets-smdjuu0paizr/backups/<file>.tar.gz`.
  The database that was there is snapshotted first and put back if the release fails.

Recordings and voicemail audio live in S3 and are not part of these backups.

## The runtime secret

`deploy/secrets.py` rebuilds `silicon-ring/production` from local files every time, so any key the
1.0 services do not read (IAM, Honeycomb, telemetry, organisations, test realms) disappears.
`deploy/render-config.py` refuses to deploy while such a key is still in the secret.

```sh
deploy/secrets.py                     # plan: key names, sources, and added/changed/removed keys
deploy/secrets.py --apply             # write it
```

| Key | Source |
|---|---|
| `RING_PUBLIC_URL`, `RING_WEB_URL`, `RING_ALLOWED_ORIGINS` | fixed: `https://backend.ring.teamofsilicons.com`, `https://ring.teamofsilicons.com`, `https://ring.teamofsilicons.com` |
| `RING_LIVEKIT_URL`, `RING_LIVEKIT_PUBLIC_URL` | fixed: `http://127.0.0.1:7880`, `wss://backend.ring.teamofsilicons.com` |
| `RING_ACCOUNTS_URL`, `RING_ACCOUNTS_APP_ID`, `RING_TING_URL` | fixed: `https://accounts.teamofsilicons.com`, `ring`, `https://backend.ting.teamofsilicons.com` |
| `RING_S3_BUCKET`, `RING_S3_PREFIX`, `AWS_REGION` | fixed: `silicon-ring-assets-smdjuu0paizr`, `production/`, `us-west-1` |
| `RING_VAPID_SUBJECT`, `RUST_LOG` | fixed: `https://ring.teamofsilicons.com`, `info` |
| `RING_LIVEKIT_API_KEY`, `RING_LIVEKIT_API_SECRET`, `RING_MEDIA_SECRET`, `RING_ENCRYPTION_KEY` | generated on the first 1.0 write, then kept |
| `RING_ACCOUNTS_APP_SECRET` | `secrets/platform/ring-app.private.json` (`app_secret`) |
| `RING_ACCOUNTS_WEBHOOK_SECRET` | the `whsec_…` value in `secrets/platform/accounts-webhook.private.json` (saved `silicon-apps webhook ring set\|rotate --json` output); kept from the current secret when that file is absent |
| `OPENAI_API_KEY`, `DEEPGRAM_API_KEY`, `RING_APNS_KEY_ID`, `RING_APNS_TEAM_ID`, `RING_APNS_BUNDLE_ID` | `secrets/dev.env` |
| `RING_APNS_KEY_BASE64` | `secrets/AuthKey_<key id>.p8` |
| `RING_FCM_SERVICE_ACCOUNT_BASE64` | `secrets/firebase-service-account.json` |
| `RING_VAPID_PRIVATE_KEY`, `RING_VAPID_PUBLIC_KEY` | `secrets/vapid/{private,public}.b64url` |

The host adds `RING_BIND=127.0.0.1:8765`, `RING_DATA_DIR=/var/lib/ring`,
`RING_MEDIA_DATA_DIR=/var/lib/ring/media` and `RING_MEDIA_SERVER_URL=ws://127.0.0.1:8765/internal/media`
itself, because the units and the Caddyfile depend on them.

A secret change reaches the host on the next install. To apply one without new code, run
`deploy/deploy.py release` again: the same tree gives the same release id, and the installer
re-renders the configuration and restarts what changed.

### Rotating secrets

| Secret | How | Effect |
|---|---|---|
| LiveKit key pair | `deploy/secrets.py --rotate RING_LIVEKIT_API_KEY --rotate RING_LIVEKIT_API_SECRET --apply`, then deploy | LiveKit restarts: calls in progress drop. Deploy when no call is active. |
| Media secret | `--rotate RING_MEDIA_SECRET --apply`, then deploy | ring-server and ring-media restart together; active representatives restart. |
| Encryption key | `--rotate RING_ENCRYPTION_KEY --apply`, then deploy | Every sealed Accounts refresh token becomes unreadable: everyone is signed out. Only after a leak. |
| Accounts app secret | `silicon-apps authors ring rotate-secret --json`, save `app_secret` into `secrets/platform/ring-app.private.json` (0600), `deploy/secrets.py --apply`, deploy | The old secret stops at once, so sign-ins fail until the deploy finishes. |
| Accounts webhook secret | `silicon-apps webhook ring rotate --json > secrets/platform/accounts-webhook.private.json` (with `umask 077`), `deploy/secrets.py --apply`, deploy | Deliveries fail until the deploy; replay them afterwards (below). |
| APNs, FCM, VAPID, OpenAI, Deepgram | replace the file or the value in `secrets/`, `deploy/secrets.py --apply`, deploy | A new VAPID pair invalidates every browser's Web Push subscription until the web app re-subscribes. |

## Silicon Accounts webhook

Ring keeps names, photos and ids current, and ends sessions, from the Accounts webhook at
`https://backend.ring.teamofsilicons.com/v1/accounts/webhook` (HMAC-signed; ring-server verifies it
with `RING_ACCOUNTS_WEBHOOK_SECRET`).

```sh
umask 077
silicon-apps webhook ring set https://backend.ring.teamofsilicons.com/v1/accounts/webhook \
  --idempotency-key "$(cat secrets/platform/webhook-set-key.txt 2>/dev/null || uuidgen | tee secrets/platform/webhook-set-key.txt)" \
  --json > secrets/platform/accounts-webhook.private.json   # the whsec_ secret is shown once
deploy/secrets.py --apply && deploy/deploy.py release
silicon-apps webhook ring show --json
silicon-accounts app --app-id ring webhook test                         # needs the app secret (app use ring --secret-stdin)
silicon-accounts app --app-id ring webhook deliveries --status failed
silicon-accounts app --app-id ring webhook replay …                    # after an outage or a rotation
```

The default events are what Ring needs: `id_change`, `display_name_change`, `pfp_change`,
`access_removed` (sign-outs and removed access) and `account_deleted`.

The sign-in setup (methods, redirect URIs, branding) is in `secrets/platform/signin-patch.json`; its
redirect URIs are listed in [`udd/accounts.md`](../udd/accounts.md).

## Ting notification types

ring-server notifies silicons through Ting with 13 types, listed with their descriptions in
`deploy/ting-types.json`. Type names are permanent in Ting; only descriptions change.

```sh
silicon-accounts login --app ting -q | ting login --token-stdin   # once, as si:tos
deploy/ting-types.py                     # plan: register / update / unchanged / legacy
deploy/ting-types.py --apply             # register new types, refresh descriptions
deploy/ting-types.py --apply --retire    # also mark pre-1.0 types (ring.session.revoked, …) as retired
```

A send to an unregistered type fails, so register a new type before deploying the server that
sends it.

## The ring CLI on Silicon Apps and GitHub

```sh
scripts/build-cli.sh                 # six targets + macOS universal → dist/cli/ (+ SHA256SUMS)
scripts/package-apps.py              # linux-x86_64, linux-aarch64 → dist/apps/ (validate + pack, local)
scripts/publish-apps.py plan         # the exact command sequence; changes nothing
scripts/publish-apps.py preflight    # read-only: signed in as si:tos, targets live, app exists
scripts/publish-apps.py all          # upload → release → promote → readiness → publish → verify
```

Each mutation keeps its idempotency key, request and result under
`secrets/platform/apps-<version>-<step>-*`, so a step that ended uncertainly is retried with the
same key, and a finished step is never repeated. Versions are immutable on Apps: a fix needs a new
version.

macOS and Windows silicons install from GitHub Releases (`install.sh` on the web app fetches the
archive and checks it against `SHA256SUMS`). Pushing a tag `v<workspace version>` runs
`.github/workflows/release.yml`, which builds the CLI for six targets, the Linux app and the Windows
app, and creates a **draft** release with every asset and `SHA256SUMS`. Review the draft, then
publish it on GitHub.

## Changing the stack

```sh
deploy/deploy.py stack                     # create a change set from deploy/stack.yaml and print it
deploy/deploy.py stack --execute           # apply it
deploy/deploy.py stack --new-ami --execute # move the host to the latest AL2023 arm64 image
```

`deploy.py stack` pins the image the running instance came from (`PinnedAmi`), so an update that
only touches, say, the security group never replaces the host. Read the change set before
executing: `Replacement=True` on `Instance` means a new host and an empty `/var/lib/ring`. In that
case run `deploy/deploy.py backup` first, then after the update, once the instance shows up in
`aws ssm describe-instance-information`, run `deploy/deploy.py release --restore-backup s3://…`.

The bucket and the Elastic IP are kept across replacement (`DeletionPolicy: Retain` on the bucket;
the EIP association moves to the new instance), so DNS (Namecheap: `ring.` and `backend.ring.` →
13.57.4.48) never changes.

## Moving to 1.0 (one time)

The pre-1.0 host is an x86_64 t3.small with a different secret and layout. In order:

1. `silicon-apps webhook ring set …` (above) so `accounts-webhook.private.json` exists.
2. `deploy/secrets.py`, review, `deploy/secrets.py --apply`. This removes every pre-1.0 key.
3. `deploy/deploy.py backup --label pre-1-0` (the old database does not fit the 1.0 schema; the
   backup is for the record).
4. `deploy/deploy.py stack`, check that `Instance` is replaced and `Assets`/`Address` are not, then
   `--execute`.
5. Wait for SSM, then `deploy/deploy.py release` with a fresh `dist/server` and `web/out`.
6. `deploy/ting-types.py --apply --retire`.
7. Smoke test:
   ```sh
   curl -fsS https://backend.ring.teamofsilicons.com/healthz
   curl -fsS https://ring.teamofsilicons.com/v1/meta
   silicon-accounts login --app ring -q | ring login --slt-stdin
   ring login status --json && ring call init c:<you> --start "Hello from Ring 1.0."
   ```
8. `scripts/publish-apps.py all`, then tag `v1.0.0` for the GitHub release.

## Troubleshooting

| Symptom | Look at |
|---|---|
| `deploy.py release` says another deploy is running | a previous SSM command still holds `/run/ring-deploy.lock`; check `aws ssm list-commands` |
| Install fails at "https gateway" on a new host | `deploy.py logs --unit caddy`: ports 80/443 reachable, DNS pointing at 13.57.4.48, ACME rate limits |
| Calls connect but there is no audio | security group ports 7881/tcp, 7882–7892/udp, 3478/udp; `deploy.py logs --unit livekit` should show 13.57.4.48 as the external or node IP (`found external IP via STUN`, then `using external IPs` or `using node IP for NAT1To1Ips`). If it shows anything else, replace `use_external_ip: true` with `node_ip: 13.57.4.48` in `deploy/livekit.yaml.tmpl` and deploy |
| `ring-media` restarts in a loop | `deploy.py logs --unit ring-media`; it needs ring-server's `/internal/media` and LiveKit on 7880 |
| Silicons get no call notifications | `ring notifications status` on the silicon; `deploy/ting-types.py` shows unregistered types; `deploy.py logs --unit ring-server` for Ting errors |
| Names or photos are stale, or sign-outs do not apply | `silicon-accounts app --app-id ring webhook deliveries --status failed` |
