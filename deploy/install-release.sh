#!/usr/bin/env bash
# Installs, rolls back or reports a Silicon Ring release on the production host.
# Runs as root over SSM (deploy/deploy.py), never interactively, and prints no secret values.
#
#   install-release.sh install  --id ID --archive s3://BUCKET/releases/ID.tar.gz --sha256 HEX \
#                               --secret-id ARN --bucket BUCKET [--region R] [--restore-backup S3URI]
#   install-release.sh rollback --secret-id ARN --bucket BUCKET [--to ID] [--restore-db] [--region R]
#   install-release.sh status
#
# Layout: /opt/ring/releases/<id>/{bin,web,deploy,RELEASE.json}, /opt/ring/current and
# /opt/ring/previous symlinks, pinned Caddy and LiveKit under /opt/ring/tools/<name>-<version>/
# (linked into each release's bin/), state in /var/lib/ring, snapshots in /var/backups/ring.
set -euo pipefail
umask 022
export PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin

readonly CADDY_VERSION=2.11.7
readonly CADDY_SHA256=d8fc6d179a5d283028a472a5618564f6ad8a86fed513e64f032b3b0b7cc45e42
readonly LIVEKIT_VERSION=1.13.10
readonly LIVEKIT_SHA256=ff60607e1931ff078576882fa11c087aa5821d635ac0c47f56ff77e203e32006

readonly ROOT=/opt/ring
readonly RELEASES=$ROOT/releases
readonly TOOLS=$ROOT/tools
readonly DATA=/var/lib/ring
readonly BACKUPS=/var/backups/ring
readonly KEEP_RELEASES=5
readonly KEEP_SNAPSHOTS=10
readonly WEB_HOST=ring.teamofsilicons.com
readonly API_HOST=backend.ring.teamofsilicons.com
readonly UNITS=(livekit.service ring-server.service ring-media.service caddy.service ring-backup.service ring-backup.timer)
readonly MANAGED=(/etc/ring/server.env /etc/ring/media.env /etc/livekit/livekit.yaml /etc/caddy/Caddyfile)

log() { printf '[ring-install] %s\n' "$*"; }
die() { printf '[ring-install] error: %s\n' "$*" >&2; exit 1; }

REGION=us-west-1
ID='' ARCHIVE='' SHA256='' SECRET_ID='' BUCKET='' RESTORE_BACKUP='' TO='' RESTORE_DB=0

# State for rollback-on-failure inside activate().
SAVED='' PREVIOUS='' OLD_PREVIOUS='' SNAPSHOT='' SWITCHED=0 SERVER_RESTARTED=0 LIVEKIT_RESTARTED=0

ensure_layout() {
  [[ "$(uname -m)" == aarch64 ]] || die 'releases are built for aarch64 hosts'
  local user
  for user in ring caddy livekit; do
    id "$user" >/dev/null 2>&1 || useradd --system --home-dir "/var/lib/$user" --shell /sbin/nologin "$user"
  done
  install -d -m 0700 /etc/ring "$BACKUPS"
  install -d -m 0750 -g livekit /etc/livekit
  install -d -m 0755 /etc/caddy "$ROOT" "$RELEASES" "$TOOLS"
  install -d -m 0700 -o ring -g ring "$DATA"
  install -d -m 0700 -o caddy -g caddy /var/lib/caddy
  install -d -m 0700 -o livekit -g livekit /var/lib/livekit
}

valid_id() { [[ "$1" =~ ^[0-9A-Za-z][0-9A-Za-z._-]{0,127}$ ]] || die "invalid release id: $1"; }

# Download a pinned upstream archive once, check its SHA-256 and keep the binary under /opt/ring/tools.
ensure_tool() {
  local name=$1 version=$2 sha=$3 url=$4 member=$5
  local dir="$TOOLS/$name-$version"
  [[ -x "$dir/$member" ]] && return 0
  log "installing $name $version"
  local tmp
  tmp=$(mktemp -d)
  curl --fail --silent --show-error --location --retry 3 --proto '=https' --tlsv1.2 -o "$tmp/archive.tar.gz" "$url"
  echo "$sha  $tmp/archive.tar.gz" | sha256sum --check --quiet - || { rm -rf "$tmp"; die "$name archive checksum mismatch"; }
  tar -xzf "$tmp/archive.tar.gz" -C "$tmp" "$member"
  rm -rf "$dir.partial"
  install -d -m 0755 "$dir.partial"
  install -m 0755 "$tmp/$member" "$dir.partial/$member"
  mv -T "$dir.partial" "$dir"
  rm -rf "$tmp"
}

fetch_release() {
  local release="$RELEASES/$ID"
  if [[ -f "$release/RELEASE.json" ]]; then
    log "release $ID is already on this host"
    return 0
  fi
  local work staging
  work=$(mktemp -d /var/tmp/ring-release.XXXXXXXX)
  log "downloading $ARCHIVE"
  aws s3 cp "$ARCHIVE" "$work/release.tar.gz" --region "$REGION" --only-show-errors
  echo "$SHA256  $work/release.tar.gz" | sha256sum --check --quiet - || { rm -rf "$work"; die 'release archive checksum mismatch'; }
  staging=$(mktemp -d "$RELEASES/.staging-$ID.XXXXXX")
  python3 -I - "$work/release.tar.gz" "$staging" <<'PY'
import pathlib, sys, tarfile
archive, destination = sys.argv[1:]
root = pathlib.Path(destination)
allowed = {'bin', 'web', 'deploy', 'RELEASE.json'}
with tarfile.open(archive, 'r:gz') as tar:
    members = tar.getmembers()
    for m in members:
        parts = pathlib.PurePosixPath(m.name).parts
        if m.name.startswith('/') or '..' in parts or not parts or parts[0] not in allowed or not (m.isfile() or m.isdir()):
            raise SystemExit(f'unexpected entry in the release archive: {m.name}')
    tar.extractall(root, members=members)
required = ['RELEASE.json', 'bin/ring-server', 'bin/ring-media', 'web/index.html', 'deploy/Caddyfile',
            'deploy/livekit.yaml.tmpl', 'deploy/install-release.sh', 'deploy/render-config.py', 'deploy/backup.py',
            'deploy/systemd/ring-server.service', 'deploy/systemd/ring-media.service', 'deploy/systemd/livekit.service',
            'deploy/systemd/caddy.service', 'deploy/systemd/ring-backup.service', 'deploy/systemd/ring-backup.timer']
missing = [p for p in required if not (root / p).is_file()]
if missing:
    raise SystemExit('the release archive is missing: ' + ', '.join(missing))
for path in [root, *root.rglob('*')]:
    executable = path.is_dir() or path.parent == root / 'bin' or path.suffix == '.sh'
    path.chmod(0o755 if executable else 0o644)
PY
  chown -R root:root "$staging"
  mv -T "$staging" "$release"
  rm -rf "$work"
  log "extracted $ID"
}

link_tools() {
  local release=$1
  ensure_tool caddy "$CADDY_VERSION" "$CADDY_SHA256" \
    "https://github.com/caddyserver/caddy/releases/download/v$CADDY_VERSION/caddy_${CADDY_VERSION}_linux_arm64.tar.gz" caddy
  ensure_tool livekit "$LIVEKIT_VERSION" "$LIVEKIT_SHA256" \
    "https://github.com/livekit/livekit/releases/download/v$LIVEKIT_VERSION/livekit_${LIVEKIT_VERSION}_linux_arm64.tar.gz" livekit-server
  ln -sfn "$TOOLS/caddy-$CADDY_VERSION/caddy" "$release/bin/caddy"
  ln -sfn "$TOOLS/livekit-$LIVEKIT_VERSION/livekit-server" "$release/bin/livekit-server"
}

render_config() {
  local release=$1
  log 'rendering configuration from the runtime secret'
  aws secretsmanager get-secret-value --region "$REGION" --secret-id "$SECRET_ID" --query SecretString --output text \
    | python3 -I "$release/deploy/render-config.py" --template "$release/deploy/livekit.yaml.tmpl" \
      --bucket "$BUCKET" --region "$REGION"
}

save_managed() {
  SAVED=$(mktemp -d "$BACKUPS/config.XXXXXXXX")
  local file unit
  for file in "${MANAGED[@]}"; do
    [[ ! -f "$file" ]] || cp -p "$file" "$SAVED/$(basename "$file")"
  done
  for unit in "${UNITS[@]}"; do
    [[ ! -f "/etc/systemd/system/$unit" ]] || cp -p "/etc/systemd/system/$unit" "$SAVED/$unit"
  done
}

restore_managed() {
  local file unit
  for file in "${MANAGED[@]}"; do
    [[ ! -f "$SAVED/$(basename "$file")" ]] || cp -p "$SAVED/$(basename "$file")" "$file"
  done
  for unit in "${UNITS[@]}"; do
    [[ ! -f "$SAVED/$unit" ]] || cp -p "$SAVED/$unit" "/etc/systemd/system/$unit"
  done
}

http_ok() { curl --fail --silent --show-error --max-time 4 -o /dev/null "$@" 2>/dev/null; }

wait_for() {
  local what=$1 seconds=$2
  shift 2
  local i
  for ((i = 0; i < seconds; i += 2)); do
    if "$@"; then
      log "$what is healthy"
      return 0
    fi
    sleep 2
  done
  printf '[ring-install] %s did not become healthy within %ss\n' "$what" "$seconds" >&2
  return 1
}

media_stable() {
  local before after
  before=$(systemctl show -p NRestarts --value ring-media)
  sleep 8
  after=$(systemctl show -p NRestarts --value ring-media)
  systemctl is-active --quiet ring-media && [[ "$before" == "$after" ]]
}

https_ok() {
  http_ok --resolve "$API_HOST:443:127.0.0.1" "https://$API_HOST/healthz" &&
    http_ok --resolve "$WEB_HOST:443:127.0.0.1" "https://$WEB_HOST/" &&
    http_ok --resolve "$WEB_HOST:443:127.0.0.1" "https://$WEB_HOST/v1/meta"
}

file_sum() { if [[ -e "$1" ]]; then sha256sum "$(readlink -f "$1")" | cut -d' ' -f1; fi; }

restore_database() {
  [[ -n "$SNAPSHOT" && -f "$SNAPSHOT/manifest.json" ]] || return 0
  systemctl stop ring-media ring-server || true
  local failed="$SNAPSHOT/failed" db
  install -d -m 0700 "$failed"
  for db in "$DATA"/*.sqlite3 "$DATA"/*.sqlite3-wal "$DATA"/*.sqlite3-shm; do
    [[ ! -e "$db" ]] || mv "$db" "$failed/"
  done
  python3 -I - "$SNAPSHOT" "$DATA" <<'PY'
import json, os, pathlib, pwd, shutil, sys
snapshot, data = map(pathlib.Path, sys.argv[1:])
owner = pwd.getpwnam('ring')
for entry in json.loads((snapshot / 'manifest.json').read_text())['databases']:
    target = data / entry['name']
    shutil.copy2(snapshot / entry['name'], target)
    os.chown(target, owner.pw_uid, owner.pw_gid)
    target.chmod(0o600)
PY
  log "restored the database from $SNAPSHOT (the failed state is in $failed)"
}

on_failure() {
  local status=$?
  trap - EXIT
  [[ $status -ne 0 ]] || return 0
  printf '[ring-install] activation failed; restoring the previous state\n' >&2
  if [[ -n "$SAVED" ]]; then restore_managed; fi
  if [[ $SWITCHED -eq 1 && -n "$PREVIOUS" ]]; then
    ln -sfn "$PREVIOUS" "$ROOT/current.next"
    mv -Tf "$ROOT/current.next" "$ROOT/current"
    if [[ -n "$OLD_PREVIOUS" ]]; then
      ln -sfn "$OLD_PREVIOUS" "$ROOT/previous.next"
      mv -Tf "$ROOT/previous.next" "$ROOT/previous"
    fi
  fi
  systemctl daemon-reload || true
  if [[ $SWITCHED -eq 0 ]]; then
    # Nothing was restarted yet, so the running services never saw the new configuration.
    printf '[ring-install] nothing was switched or restarted\n' >&2
    exit "$status"
  fi
  if [[ $SERVER_RESTARTED -eq 1 ]]; then restore_database; fi
  if [[ -n "$PREVIOUS" ]]; then
    if [[ $LIVEKIT_RESTARTED -eq 1 ]]; then systemctl restart livekit || true; fi
    systemctl restart ring-server ring-media || true
    systemctl reload-or-restart caddy || true
    if wait_for ring-server 30 http_ok http://127.0.0.1:8765/healthz; then
      printf '[ring-install] back on %s\n' "$(basename "$PREVIOUS")" >&2
    else
      printf '[ring-install] the previous release is not healthy either; inspect with deploy.py logs\n' >&2
    fi
  else
    printf '[ring-install] there is no previous release to return to\n' >&2
  fi
  exit "$status"
}

prune() {
  local current previous
  current=$(readlink -e "$ROOT/current" || true)
  previous=$(readlink -e "$ROOT/previous" || true)
  local count=0 dir
  while IFS= read -r dir; do
    [[ -n "$dir" ]] || continue
    count=$((count + 1))
    if ((count > KEEP_RELEASES)) && [[ "$dir" != "$current" && "$dir" != "$previous" ]]; then
      rm -rf -- "$dir"
    fi
  done < <(find "$RELEASES" -mindepth 1 -maxdepth 1 -type d ! -name '.*' -printf '%T@ %p\n' | sort -rn | cut -d' ' -f2-)
  local tool used
  for tool in "$TOOLS"/*/; do
    tool=${tool%/}
    [[ -d "$tool" ]] || continue
    used=$(find "$RELEASES" -mindepth 3 -maxdepth 3 -path '*/bin/*' -lname "$tool/*" -print -quit)
    [[ -n "$used" ]] || rm -rf -- "$tool"
  done
  find "$RELEASES" "$TOOLS" -mindepth 1 -maxdepth 1 -name '.*' -mmin +60 -exec rm -rf -- {} +
  keep_newest "$KEEP_SNAPSHOTS" "$BACKUPS"/pre-*
  keep_newest "$KEEP_SNAPSHOTS" "$BACKUPS"/config.*
}

# Remove all but the newest N of the given paths. An unmatched glob arrives as a literal pattern,
# which ls reports as missing; that is fine here.
keep_newest() {
  local keep=$1
  shift
  # Names under $BACKUPS are generated here and contain no whitespace.
  # shellcheck disable=SC2012
  { ls -1dt "$@" 2>/dev/null || true; } | tail -n +$((keep + 1)) | xargs -r rm -rf --
}

# Switch /opt/ring/current to a release, restart what changed in dependency order, health-check,
# and on any failure put the previous release, configuration and database back.
activate() {
  local id=$1
  local release="$RELEASES/$id"
  [[ -f "$release/RELEASE.json" ]] || die "release $id is not on this host"
  # On failure, go back to whatever was current (the same release when re-applying it with new
  # configuration), and put /opt/ring/previous back as it was.
  PREVIOUS=$(readlink -e "$ROOT/current" || true)
  OLD_PREVIOUS=$(readlink -e "$ROOT/previous" || true)

  link_tools "$release"
  local old_livekit old_caddy old_lk_conf old_caddyfile
  old_livekit=$(file_sum "$ROOT/current/bin/livekit-server")
  old_caddy=$(file_sum "$ROOT/current/bin/caddy")
  old_lk_conf=$(file_sum /etc/livekit/livekit.yaml)
  old_caddyfile=$(file_sum /etc/caddy/Caddyfile)

  save_managed
  trap on_failure EXIT
  render_config "$release"
  install -m 0644 "$release/deploy/Caddyfile" /etc/caddy/Caddyfile
  local unit
  for unit in "${UNITS[@]}"; do
    install -m 0644 "$release/deploy/systemd/$unit" "/etc/systemd/system/$unit"
  done
  runuser -u caddy -- env HOME=/var/lib/caddy XDG_DATA_HOME=/var/lib/caddy/data XDG_CONFIG_HOME=/var/lib/caddy/config \
    "$release/bin/caddy" validate --config /etc/caddy/Caddyfile --adapter caddyfile >/dev/null 2>&1 ||
    die 'the Caddyfile does not validate'

  if compgen -G "$DATA/*.sqlite3" >/dev/null; then
    SNAPSHOT="$BACKUPS/pre-$id-$(date -u +%Y%m%dT%H%M%SZ)"
    python3 -I "$release/deploy/backup.py" --data-dir "$DATA" snapshot --dest "$SNAPSHOT" >/dev/null
    log "database snapshot: $SNAPSHOT"
  fi

  ln -sfn "$release" "$ROOT/current.next"
  mv -Tf "$ROOT/current.next" "$ROOT/current"
  SWITCHED=1
  if [[ -n "$PREVIOUS" && "$PREVIOUS" != "$release" ]]; then
    ln -sfn "$PREVIOUS" "$ROOT/previous.next"
    mv -Tf "$ROOT/previous.next" "$ROOT/previous"
  fi
  log "current -> $id"

  systemctl daemon-reload
  systemctl enable --quiet livekit.service ring-server.service ring-media.service caddy.service ring-backup.timer

  # LiveKit restarts drop every call in progress, so only when its binary or config changed.
  if [[ "$(file_sum "$release/bin/livekit-server")" != "$old_livekit" || "$(file_sum /etc/livekit/livekit.yaml)" != "$old_lk_conf" ]] ||
    ! systemctl is-active --quiet livekit; then
    systemctl restart livekit
    LIVEKIT_RESTARTED=1
  fi
  wait_for livekit 40 http_ok http://127.0.0.1:7880/

  if [[ -n "$RESTORE_BACKUP" ]]; then
    # Whatever was there is in $SNAPSHOT, and on_failure puts it back.
    SERVER_RESTARTED=1
    systemctl stop ring-media ring-server || true
    python3 -I "$release/deploy/backup.py" --data-dir "$DATA" --region "$REGION" restore --from "$RESTORE_BACKUP" --replace
  fi

  SERVER_RESTARTED=1
  systemctl restart ring-server
  wait_for ring-server 60 http_ok http://127.0.0.1:8765/healthz
  systemctl restart ring-media
  wait_for ring-media 20 media_stable

  if [[ "$(file_sum "$release/bin/caddy")" != "$old_caddy" ]] || ! systemctl is-active --quiet caddy; then
    systemctl restart caddy
  elif [[ "$(file_sum /etc/caddy/Caddyfile)" != "$old_caddyfile" ]]; then
    systemctl reload caddy
  fi
  # A fresh host obtains its certificates here, which can take a minute.
  wait_for 'https gateway' 150 https_ok

  systemctl start ring-backup.timer
  trap - EXIT
  rm -rf "$SAVED"
  prune
  log "release $id is live"
}

cmd_install() {
  [[ -n "$ID" && -n "$ARCHIVE" && -n "$SHA256" && -n "$SECRET_ID" && -n "$BUCKET" ]] ||
    die 'install needs --id, --archive, --sha256, --secret-id and --bucket'
  valid_id "$ID"
  [[ "$SHA256" =~ ^[0-9a-f]{64}$ ]] || die 'invalid --sha256'
  [[ "$ARCHIVE" == s3://* ]] || die '--archive must be an s3:// URI'
  ensure_layout
  fetch_release
  activate "$ID"
}

cmd_rollback() {
  [[ -n "$SECRET_ID" && -n "$BUCKET" ]] || die 'rollback needs --secret-id and --bucket'
  local current target
  current=$(readlink -e "$ROOT/current" || true)
  if [[ -n "$TO" ]]; then
    valid_id "$TO"
    target="$RELEASES/$TO"
  else
    target=$(readlink -e "$ROOT/previous" || true)
  fi
  [[ -n "$target" && -d "$target" ]] || die 'no release to roll back to'
  [[ "$target" != "$current" ]] || die "$(basename "$target") is already current"
  if [[ $RESTORE_DB -eq 1 ]]; then
    local snapshot
    # shellcheck disable=SC2012
    snapshot=$(ls -1dt "$BACKUPS/pre-$(basename "$current")-"* 2>/dev/null | head -n 1 || true)
    [[ -n "$snapshot" ]] || die "no snapshot from before $(basename "$current") was installed"
    RESTORE_BACKUP=''
    systemctl stop ring-media ring-server || true
    SNAPSHOT=$snapshot
    restore_database
    SNAPSHOT=''
  fi
  ensure_layout
  activate "$(basename "$target")"
}

cmd_status() {
  local current previous unit
  current=$(readlink -e "$ROOT/current" || true)
  previous=$(readlink -e "$ROOT/previous" || true)
  printf 'current:  %s\nprevious: %s\n' "${current:+$(basename "$current")}" "${previous:+$(basename "$previous")}"
  [[ -z "$current" ]] || python3 -I -c 'import json,sys; d=json.load(open(sys.argv[1])); print("version:  %s (%s)" % (d.get("version"), d.get("source_revision", "")[:12]))' "$current/RELEASE.json"
  printf 'releases: %s\n' "$(find "$RELEASES" -mindepth 1 -maxdepth 1 -type d ! -name '.*' -printf '%f ' 2>/dev/null)"
  for unit in livekit ring-server ring-media caddy ring-backup.timer; do
    printf '%-18s %s\n' "$unit" "$(systemctl is-active "$unit" 2>/dev/null || true)"
  done
  printf 'ring-server /healthz: %s\n' "$(curl -s --max-time 3 http://127.0.0.1:8765/healthz || echo unreachable)"
  printf 'livekit /:            %s\n' "$(curl -s --max-time 3 http://127.0.0.1:7880/ || echo unreachable)"
  if https_ok; then echo 'https gateway:        ok'; else echo 'https gateway:        failing'; fi
}

main() {
  [[ $EUID -eq 0 ]] || die 'run as root'
  local command=${1:-}
  shift || true
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --id) ID=$2; shift 2 ;;
      --archive) ARCHIVE=$2; shift 2 ;;
      --sha256) SHA256=$2; shift 2 ;;
      --secret-id) SECRET_ID=$2; shift 2 ;;
      --bucket) BUCKET=$2; shift 2 ;;
      --region) REGION=$2; shift 2 ;;
      --restore-backup) RESTORE_BACKUP=$2; shift 2 ;;
      --to) TO=$2; shift 2 ;;
      --restore-db) RESTORE_DB=1; shift ;;
      *) die "unknown argument: $1" ;;
    esac
  done
  exec 9>/run/ring-deploy.lock
  flock -n 9 || die 'another deploy is running on this host'
  case "$command" in
    install) cmd_install ;;
    rollback) cmd_rollback ;;
    status) cmd_status ;;
    *) die 'usage: install-release.sh install|rollback|status [options]' ;;
  esac
}

main "$@"
