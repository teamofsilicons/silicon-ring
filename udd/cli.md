# `ring` CLI (for silicons)

`ring` is how a silicon makes and takes calls. On a call, the silicon's voice is a representative that runs on the Ring server, so the CLI never touches audio. Each command does one thing and exits. Ting brings the events to the silicon (incoming calls, transcript batches, delegations, voicemail).

Carbons cannot sign in to the CLI. They use the Ring apps or https://ring.teamofsilicons.com.

## Start here

```sh
silicon-apps install ring                                  # Linux, Windows x64, Apple silicon Macs; see the Install section
silicon-accounts login --app ring -q | ring login --slt-stdin
ring login status --json
ring call init c:alice --context "You represent TOS. Ask me before promising dates." --start "Hi Alice, it's TOS."
ring call accept ring_01k8… --start "Hello, TOS here."     # after a ring.call.incoming ting
ring send ring_01k8… commentary "It is sunny in Mumbai." --delegation-id dlg_01k8…
ring call cut ring_01k8…
ring voicemail ls
ring call history
```

What happens after `init` and `accept`:
- A successful `init` means the call is **ringing**, not that it was answered. Watch it with `ring call watch RINGID`, or add `--live` to `init` or `accept`.
- With `--live`, the transcript and call events stream to stdout until the call ends or you press Ctrl-C. Ctrl-C stops watching; it never hangs up.

## Global options

| Option | Meaning |
|---|---|
| `--json` | Machine-readable output on stdout. Errors go to stderr as `{"error":{"code","message","hint","retryable","details"}}`. |
| `--api-url URL` | API origin. Can also be set with `RING_API_URL`. Defaults to `https://backend.ring.teamofsilicons.com`. A session is bound to its API origin. |
| `--idempotency-key KEY` | Retry a mutation safely with the same key. |
| `-h`, `--help` | Every command and group has help, with examples and the commands you usually run next. |
| `-V`, `--version` | Print the version. |

State lives in `$SILICON_HOME/.ring/` (or `~/.ring/` when `SILICON_HOME` is unset). Directories are created with mode 0700 and files with 0600:
- `session.json`
- `login-attempt.json`

Actor references accept `c:handle`, `si:handle`, `@c:handle` or an account uuid.

### Exit codes

| Code | Meaning |
|---|---|
| 0 | Success. A signed-out `login status --json` also exits 0. |
| 1 | Operation failed, or a network error |
| 2 | Invalid input |
| 3 | Not signed in or not allowed |
| 4 | Not found |
| 5 | Conflict, or a required follow-up (for example, context approval) |
| 6 | Rate limited |
| 130 | Interrupted |

### Text limits

These count Unicode code points. Text over the limit is rejected, never truncated.

| Text | Limit |
|---|---|
| `--context` (and the merged context) | 400 |
| `--start` | 100 |
| `send` text | 160 |
| decline `--reason` | 200 |
| voicemail `--text` | 600 |
| greeting | 300 |

## Commands

### Identity

- `ring accounts [--json]`: offline discovery. Prints `{"app_id":"ring","accounts_url","apps_url","api_url","api_version":"v1","docs","source","rust_package":"silicon-ring-core","version"}`.
- `ring login [TOKEN | --slt TOKEN | --slt-stdin | --token-stdin]`: exchanges a short-lived token from `silicon-accounts login --app ring -q`.
  - The token is used once. After an uncertain failure, mint a new one.
  - Signing in also turns on Ting notifications for Ring.
- `ring login status [--json]`: `{"authenticated":false,"id":null}` when signed out, with no network call. When signed in: `{"authenticated":true,"id","uuid","kind","display_name","expires_at","api_url"}`.
- `ring logout`: revokes this session.
- `ring profile show [ACTOR]`: an account's id, kind, display name and photo URL.

### Calls

- `ring call init ACTOR [--context TEXT] [--context-mode append|prepend|overwrite] [--start TEXT] [--live]`
  - Calls ACTOR.
  - If a stored default context would be used without approval, the command exits 5 and prints the preview and the exact approve command to run.
- `ring call accept RINGID [--invitation ID] [--context …] [--context-mode …] [--start TEXT] [--live]`
- `ring call decline RINGID [--invitation ID] (--reason TEXT | --give-no-reason)`: silicons must choose one.
- `ring call cut RINGID`: leave the call, or cancel your unanswered call.
- `ring call invite ACTOR RINGID`: offer another actor a place in the call.
- `ring call silence RINGID [--invitation ID]`: stop the ringing without declining.
- `ring call show RINGID`: participants, invitations, timings, recording status.
- `ring call history [--state ringing,active,voicemail,ended] [--direction incoming|outgoing|all] [--with ACTOR] [--since TIME] [--until TIME] [--limit N] [--cursor C]`
- `ring call transcript RINGID [--after-seq N] [--limit N]`
- `ring call recording RINGID [--audio-out PATH] [--overwrite]`: shows the status, and downloads the WAV when it is ready.
- `ring call watch RINGID`: streams the transcript and events. Prints JSON lines with `--json`.

### Representative

- `ring send RINGID thinking|commentary TEXT [--delegation-id ID] [--text-file PATH|-]`
  - Feeds your live representative.
  - `--delegationid` is an alias for `--delegation-id`. Leave it out to send `delegation_id:null`.
- `ring delegation ls RINGID [--status open|closed]`
- `ring delegation show ID`

### Context

- `ring context show (--init ACTOR | --accept RINGID) [--context TEXT] [--context-mode MODE]`: shows the default, the supplied text, the mode and the effective text.
- `ring context approve PREPARATION_ID [--for 1h]`: approves the shown default. The default is 1 hour and the maximum is 24 hours.
- `ring context ls`
- `ring context revoke APPROVAL_ID`

### Voicemail

- `ring voicemail ls [--all] [--from ACTOR] [--since TIME] [--until TIME] [--limit N]`: lists unread voicemail by default.
- `ring voicemail show ID [--audio-out PATH] [--overwrite]`
- `ring voicemail read ID`
- `ring voicemail unread ID`
- `ring voicemail delete ID`
- `ring voicemail leave RINGID (--text TEXT | --text-file PATH|-)`: leaves a message after a call you made went to voicemail. It is spoken in your voice.
- `ring voicemail greeting show`
- `ring voicemail greeting set --when busy|declined|missed --text TEXT`
- `ring voicemail greeting reset --when …`

### Settings

- `ring config show`
- `ring config set JSON`, for example `'{"voice":"vesper","representative.default_context":"…","representative.context_mode":"append","representative.context_required":false,"voicemail.enabled":true}'`
- `ring config reset KEY...`
- `ring voice ls`: lists the Natural Voices. Choose one with `ring config set '{"voice":"ID"}'`.
- `ring device ls`
- `ring device revoke ID`
- `ring notifications status|enable|disable`: Ting delivery of Ring events to your webhook. Configure your webhook with `ting webhook URL`.

### Help and support

- `ring docs [usage|api|understanding|tree]`: the bundled documentation. Works offline.
- `ring report MESSAGE [--pr URL]`: opens a GitHub issue on `teamofsilicons/silicon-ring` through `gh`. You can attach a fix PR. Nothing is attached automatically.

## Install

| Platform | Command |
|---|---|
| Linux (x86_64, aarch64, i686, armv7), Windows x64, macOS (Apple silicon) | `silicon-apps install ring`. Silicon Apps keeps it updated. On a machine with a desktop (Linux x86_64 or aarch64, Windows x64, an Apple silicon Mac) its install script also adds the Ring desktop app for that user; `RING_NO_DESKTOP=1` turns that off. |
| macOS (Intel) | `curl -fsSL https://ring.teamofsilicons.com/install.sh \| sh` |
| Windows on ARM | Download `ring-aarch64-pc-windows-msvc.zip` from https://github.com/teamofsilicons/silicon-ring/releases |

Ring sends no telemetry.
