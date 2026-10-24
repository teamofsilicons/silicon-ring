#!/usr/bin/env bash
# The App Review demo silicon on the production host: install, sign in, start, stop, status, remove.
# Runs as root, over SSM (deploy/demo-silicon/ship.py) or in a Session Manager shell, and prints
# no secret. It never touches Ring's own releases, configuration or units.
#
#   ring-demo.sh install --from DIR   install or update the ring CLI, agent.py and ring-demo.service
#   ring-demo.sh login [--silicon ID] read a Ring short-lived token (SLT) for the demo silicon on stdin,
#                                     sign the CLI in, check it is ID (default: the configured one,
#                                     else si:ring-demo), set the voice, voicemail and declined
#                                     greeting, start the agent
#   ring-demo.sh start | stop | status
#   ring-demo.sh logout               stop the agent and revoke the CLI's Ring session
#   ring-demo.sh remove [--purge]     stop and uninstall; --purge also signs out and deletes the state
#
# Layout: /opt/ring-demo/{ring-demo.sh,agent.py,bin/ring}, /etc/systemd/system/ring-demo.service,
# /etc/ring-demo.env (RING_DEMO_SILICON, the only account the agent runs as), state in
# /var/lib/ring-demo, the home of the ring-demo system user (.ring/session.json, agent-state.json).
set -euo pipefail
umask 022
export PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin

# The ring CLI from Ring's own GitHub release, checked against this SHA-256. Fix releases replace
# assets in place; if the check fails, read the new digest with
#   gh release view v1.0.0 --repo teamofsilicons/silicon-ring --json assets \
#     --jq '.assets[] | select(.name=="ring-aarch64-unknown-linux-musl.tar.gz") | .digest'
# and update it here.
readonly RING_VERSION=v1.0.0
readonly RING_ASSET=ring-aarch64-unknown-linux-musl.tar.gz
readonly RING_SHA256=6ef2902194ad6ab3c573f6b928da413e3478edf767869f410eb3705ea6ae8d55
readonly RING_URL="https://github.com/teamofsilicons/silicon-ring/releases/download/$RING_VERSION/$RING_ASSET"

readonly ROOT=/opt/ring-demo
readonly STATE=/var/lib/ring-demo
readonly ACCOUNT=ring-demo
readonly UNIT=ring-demo.service
readonly CONF=/etc/ring-demo.env
readonly DEFAULT_SILICON=si:ring-demo
readonly API=http://127.0.0.1:8765
# Ring Demo declines only silicons (who hear no greeting) and carbons who already had 3 calls
# answered this hour, so the declined greeting is for them.
readonly SETTINGS='{"voice":"gleam","voicemail.enabled":true,"voicemail.greetings.declined":"Ring Demo has already answered a few of your calls this hour, so it is not taking this one. Please try again later, or leave a message after the beep."}'

log() { printf '[ring-demo] %s\n' "$*"; }
die() { printf '[ring-demo] error: %s\n' "$*" >&2; exit 1; }

# Runs the ring CLI as the demo's own user, with a clean environment.
as_demo() {
  runuser -u "$ACCOUNT" -- env -i HOME="$STATE" PATH=/usr/bin:/bin LANG=C.UTF-8 RING_API_URL="$API" \
    "$ROOT/bin/ring" "$@"
}

# Prints the signed-in account id, or nothing when the CLI is signed out or the check failed.
signed_in_id() {
  as_demo --json login status 2>/dev/null |
    python3 -I -c 'import json,sys; d=json.load(sys.stdin); print(d.get("id") or "" if d.get("authenticated") and d.get("kind") == "silicon" else "")' 2>/dev/null ||
    true
}

valid_silicon() { [[ "$1" =~ ^si:[a-z0-9_-]{3,30}$ ]]; }

# The silicon the agent must be signed in as: RING_DEMO_SILICON in $CONF, else the default.
configured_silicon() {
  local id=''
  [[ -f "$CONF" ]] && id=$(sed -n 's/^RING_DEMO_SILICON=//p' "$CONF" | tail -n 1)
  printf '%s\n' "${id:-$DEFAULT_SILICON}"
}

write_conf() {
  valid_silicon "$1" || die "not a silicon id: $1"
  printf '# Written by ring-demo.sh login: the only account ring-demo.service runs as.\nRING_DEMO_SILICON=%s\n' "$1" >"$CONF.new"
  chmod 0644 "$CONF.new"
  mv -f "$CONF.new" "$CONF"
}

install_ring() {
  local stamp="$ROOT/bin/.ring.sha256"
  if [[ -x "$ROOT/bin/ring" && -f "$stamp" && "$(cat "$stamp")" == "$RING_SHA256" ]]; then
    return 0
  fi
  log "installing the ring CLI $RING_VERSION"
  local work binary
  work=$(mktemp -d /var/tmp/ring-demo-cli.XXXXXXXX)
  curl --fail --silent --show-error --location --retry 3 --proto '=https' --tlsv1.2 \
    -o "$work/ring.tar.gz" "$RING_URL"
  echo "$RING_SHA256  $work/ring.tar.gz" | sha256sum --check --quiet - ||
    { rm -rf "$work"; die "$RING_ASSET does not match the pinned SHA-256 (see the top of this script)"; }
  install -d -m 0700 "$work/x"
  tar -xzf "$work/ring.tar.gz" -C "$work/x"
  binary=$(find "$work/x" -type f -name ring -print -quit)
  [[ -n "$binary" ]] || { rm -rf "$work"; die "$RING_ASSET has no ring binary"; }
  install -m 0755 "$binary" "$ROOT/bin/ring.new"
  mv -f "$ROOT/bin/ring.new" "$ROOT/bin/ring"
  printf '%s\n' "$RING_SHA256" >"$stamp"
  rm -rf "$work"
}

cmd_install() {
  local from=$1
  [[ -n "$from" && -f "$from/agent.py" && -f "$from/$UNIT" && -f "$from/ring-demo.sh" ]] ||
    die 'install needs --from DIR with agent.py, ring-demo.service and ring-demo.sh'
  [[ "$(uname -m)" == aarch64 ]] || die 'the pinned ring CLI is for aarch64 hosts'
  command -v python3 >/dev/null || die 'python3 is missing'
  id "$ACCOUNT" >/dev/null 2>&1 || useradd --system --home-dir "$STATE" --shell /sbin/nologin "$ACCOUNT"
  install -d -m 0700 -o "$ACCOUNT" -g "$ACCOUNT" "$STATE"
  install -d -m 0755 "$ROOT" "$ROOT/bin"
  install_ring
  install -m 0644 "$from/agent.py" "$ROOT/agent.py"
  install -m 0755 "$from/ring-demo.sh" "$ROOT/ring-demo.sh"
  install -m 0644 "$from/$UNIT" "/etc/systemd/system/$UNIT"
  [[ -f "$CONF" ]] || write_conf "$DEFAULT_SILICON"
  systemctl daemon-reload
  log "installed: $(as_demo --version), agent.py, $UNIT"
  if [[ -f "$STATE/.ring/session.json" ]]; then
    local who expected
    who=$(signed_in_id)
    expected=$(configured_silicon)
    if [[ "$who" != "$expected" ]]; then
      log "the CLI is signed in as ${who:-nobody (or the check failed)}, not $expected: the agent would refuse to run."
      log 'Next: deploy/demo-silicon/ship.py login'
      return 0
    fi
    systemctl enable --quiet "$UNIT"
    systemctl restart "$UNIT"
    log "restarted the agent (signed in as $who)"
  else
    log 'not signed in yet. Next: deploy/demo-silicon/ship.py login'
  fi
}

cmd_login() {
  local expected=${1:-$(configured_silicon)} who
  valid_silicon "$expected" || die "not a silicon id: $expected"
  [[ -x "$ROOT/bin/ring" ]] || die 'not installed; run install first'
  # Nothing may answer calls while the CLI could be signed in as someone else.
  local was_active
  was_active=$(systemctl is-active "$UNIT" 2>/dev/null || true)
  systemctl stop "$UNIT" 2>/dev/null || true
  if ! as_demo login --slt-stdin; then
    # A failed sign-in leaves the previous session as it was: keep the demo up if that one is right.
    if [[ "$was_active" == active && "$(signed_in_id)" == "$(configured_silicon)" ]]; then
      systemctl start "$UNIT"
      log 'the agent is running again on the previous session'
    fi
    die 'ring login failed (see above); mint a new token and try again'
  fi
  who=$(signed_in_id)
  if [[ "$who" != "$expected" ]]; then
    as_demo logout >/dev/null 2>&1 || true
    die "the token signed the CLI in as ${who:-nobody (or the check failed)}, not $expected. Signed out again; the agent stays stopped. Mint the token from $expected's own sign-in"
  fi
  write_conf "$expected"
  # The agent polls the API, so Ring need not send Ting notifications to this silicon.
  as_demo notifications disable >/dev/null
  as_demo config set "$SETTINGS" >/dev/null
  systemctl enable --quiet "$UNIT"
  systemctl restart "$UNIT"
  log "signed in as $who; the agent is running"
}

cmd_status() {
  printf 'agent:      %s (%s)\n' "$(systemctl is-active "$UNIT" 2>/dev/null || true)" \
    "$(systemctl is-enabled "$UNIT" 2>/dev/null || true)"
  [[ -x "$ROOT/bin/ring" ]] || { echo 'ring CLI:   not installed'; return 0; }
  printf 'ring CLI:   %s\n' "$(as_demo --version)"
  printf 'runs as:    %s\n' "$(configured_silicon)"
  as_demo --json login status 2>/dev/null | python3 -I -c '
import json, sys
d = json.load(sys.stdin)
if not d.get("authenticated"):
    print("signed in:  no (run deploy/demo-silicon/ship.py login)")
elif d.get("id") != sys.argv[1] or d.get("kind") != "silicon":
    print("signed in:  as %s, NOT %s: the agent refuses to run (run deploy/demo-silicon/ship.py login)"
          % (d.get("id"), sys.argv[1]))
else:
    print("signed in:  %s, session until %s" % (d.get("id"), d.get("expires_at")))' "$(configured_silicon)" ||
    echo 'signed in:  unknown (the ring CLI failed)'
  if [[ -f "$STATE/agent-state.json" ]]; then
    python3 -I -c '
import json, sys, time
d = json.load(open(sys.argv[1]))
due = ["%s in %ds" % (p.get("id"), max(0, p.get("due", 0) - time.time())) for p in d.get("pending", [])]
print("call backs: %s" % (", ".join(due) or "none due"))' "$STATE/agent-state.json" || true
  fi
  echo '--- recent log'
  journalctl --no-pager --output short-iso -u "$UNIT" -n 15 || true
}

cmd_logout() {
  systemctl disable --now --quiet "$UNIT" 2>/dev/null || true
  if [[ -x "$ROOT/bin/ring" ]]; then as_demo logout || true; fi
  log 'stopped and signed out'
}

cmd_remove() {
  local purge=$1
  systemctl disable --now --quiet "$UNIT" 2>/dev/null || true
  if [[ $purge -eq 1 && -x "$ROOT/bin/ring" ]]; then as_demo logout || true; fi
  rm -f "/etc/systemd/system/$UNIT"
  systemctl daemon-reload
  rm -rf -- "$ROOT"
  if [[ $purge -eq 1 ]]; then
    rm -rf -- "$STATE"
    rm -f -- "$CONF"
    userdel "$ACCOUNT" 2>/dev/null || true
    log 'removed, signed out, state deleted'
  else
    log "removed; the CLI session and state are kept in $STATE and $CONF (remove --purge deletes them)"
  fi
}

main() {
  [[ $EUID -eq 0 ]] || die 'run as root'
  local command=${1:-} from='' purge=0 silicon=''
  shift || true
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --from) [[ $# -ge 2 ]] || die '--from needs a directory'; from=$2; shift 2 ;;
      --silicon) [[ $# -ge 2 ]] || die '--silicon needs an id'; silicon=$2; shift 2 ;;
      --purge) purge=1; shift ;;
      *) die "unknown argument: $1" ;;
    esac
  done
  [[ -z "$silicon" || "$command" == login ]] || die '--silicon goes with login'
  # The demo user cannot enter root's working directory (SSM runs commands from its own).
  cd /
  exec 9>/run/ring-demo.lock
  flock -n 9 || die 'another ring-demo.sh is running'
  case "$command" in
    install) cmd_install "$from" ;;
    login) cmd_login "$silicon" ;;
    start) systemctl enable --now --quiet "$UNIT"; log 'started' ;;
    stop) systemctl disable --now --quiet "$UNIT"; log 'stopped (the CLI stays signed in; start resumes)' ;;
    status) cmd_status ;;
    logout) cmd_logout ;;
    remove) cmd_remove "$purge" ;;
    *) die 'usage: ring-demo.sh install --from DIR | login [--silicon ID] | start | stop | status | logout | remove [--purge]' ;;
  esac
}

main "$@"
