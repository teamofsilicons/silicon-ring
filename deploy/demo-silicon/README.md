# Demo silicon for App Review

App Review has one device and nobody to call. The demo silicon, **si:ring-demo** ("Ring Demo"),
runs on the production host and answers calls from people (carbons), so a reviewer can try a call
at any time. It calls each caller back once, so they can also try an incoming call and, by
declining it, voicemail. It does nothing else.

| File | What it is |
|---|---|
| `agent.py` | The agent: Python 3.9+, standard library only. It drives the `ring` CLI, signed in as the demo silicon |
| `ring-demo.service` | Its systemd unit (user `ring-demo`, sandboxed like `ring-server.service`) |
| `ring-demo.sh` | Runs on the host as root: `install`, `login`, `start`, `stop`, `status`, `logout`, `remove` |
| `ship.py` | Runs on your machine: copies the three files above to the host and runs `ring-demo.sh` there over SSM, like `deploy/deploy.py` |

No secret is stored in this folder. The host keeps the demo silicon's Ring session in
`/var/lib/ring-demo/.ring/session.json` (mode 0600, user `ring-demo`). The one-time sign-in token
passes through S3 for a few seconds (step 5).

## What it does

| When | It runs |
|---|---|
| A carbon calls it | `ring call accept RINGID --invitation ID --context "…" --context-mode overwrite --start "Hi, this is Ring Demo. Ask me anything about Ring."` |
| A silicon calls it, or it is invited to a call with no carbon on it | `ring call decline RINGID --invitation ID --reason "Ring Demo only talks with people (carbons). …"` |
| A carbon calls it after it answered 3 of their calls in the last hour | `ring call decline RINGID --invitation ID --reason "Ring Demo has already answered a few of your calls this hour. Please try again later."` |
| Its representative asks it for help (a delegation) | `ring send RINGID commentary "This is a demo, so I can't look that up. Ask me about Ring instead." --delegation-id ID`, at once |
| A call reaches 4 minutes 30 seconds | `ring send RINGID commentary "Ring Demo calls last 5 minutes, so this one ends in about 30 seconds. Say a short goodbye now."` |
| A call reaches 5 minutes, its representative fails, or no carbon is left on the call | `ring call cut RINGID` |
| A call it answered from a carbon ends | 30 s later, `ring call init <caller> --context "…" --start "Hi, it's Ring Demo calling you back, so you can see an incoming call."` |
| That call back is declined or missed | `ring voicemail leave RINGID --text "Hi, it's Ring Demo. Sorry I missed you. …"` |

Who it talks to, and for how long:
- **Only carbons.** It declines a call from a silicon, with a reason, so two representatives never
  talk to each other. It declines an invitation to a call that has no carbon on it, and hangs up
  when the last carbon leaves a call it is on.
- **At most 5 minutes a call.** The contexts say so, and 30 s before the end it asks its
  representative to say goodbye.
- **At most 3 answered calls per caller in any hour.** It declines the next ones with a reason.
  The caller then hears its declined greeting, "Ring Demo has already answered a few of your calls
  this hour, so it is not taking this one. Please try again later, or leave a message after the
  beep."

The call back follows these rules:
- Only carbons get one. A call back ending never starts another.
- At most one call back per caller every 10 minutes.
- None after a call its representative failed on, or one it left because no carbon was on it.
- Never while Ring Demo is on a call. It waits for the call to end, and gives up after 10 minutes.

The context tells the representative that it is a demo, what Ring is, that calls are recorded and
transcribed and end after 5 minutes, to keep it short, and never to ask its silicon for help. The
exact texts are at the top of `agent.py`. The limits are options of `agent.py` (`--max-call`,
`--per-hour`, `--callback-delay`, `--callback-every`, `--give-up`).

Ring Demo never invites anyone, and never calls anyone except to call back. When it is already on
a call, Ring itself tells a second caller it is busy and takes a voicemail. Nobody reads those
voicemails.

## How it works

The agent polls the Ring API every 2 s through the `ring` CLI, one command at a time:
- Not on a call: `ring call history --state ringing,active,voicemail --limit 20`, to find calls
  ringing for it.
- On a call: `ring call show RINGID` and `ring delegation ls RINGID --status open`.

Each CLI run makes one request to ring-server on `http://127.0.0.1:8765`, so DNS, TLS and Caddy are
not involved. ring-server logs no per-request lines.

It runs only as the demo silicon. `/etc/ring-demo.env` names it (`RING_DEMO_SILICON=si:ring-demo`),
and the unit passes it to `agent.py --silicon`. When the agent finds the CLI signed in, it checks
that `ring login status --json` reports that `id` and `"kind":"silicon"`. If the CLI is signed in
as anyone else, the agent logs `refusing to run` and exits 78, and systemd does not restart it.
`ring-demo.sh login` checks the same thing right after signing in. On a mismatch it signs the CLI
out again and leaves the agent stopped.

The agent survives restarts. `/var/lib/ring-demo/agent-state.json` holds:
- the call backs that are due;
- when each caller was last called back;
- when it answered each caller's calls in the last hour.

Everything else it reads back from the API, and it picks up a call that was in progress. Stopping it
never cuts a `ring` command off: systemd sends SIGTERM to the agent alone (`KillMode=mixed`), and
the agent stops between commands. A command that a signal or timeout does cut off counts as a
failure worth retrying, so a call back that was due is kept and tried again with the same
idempotency key.

It does not use Ting webhooks, which is what `deploy/appstore/review-notes.md` first planned:
- Polling needs only the `ring` binary. That binary is static (musl) and comes from Ring's own
  GitHub release.
- Ting would need its daemon, `silicon-accounts`, a Ting sign-in, a webhook secret and a
  machine-wide socket in `/var/tmp/silicon-ting`.
- The Linux builds of Ting and `silicon-accounts` link against glibc on Ubuntu 24.04 runners. They
  were not checked on the host's Amazon Linux 2023 (glibc 2.34).

Signing in turns Ting notifications off for the demo silicon, since nothing would read them.

On the host:

```
/opt/ring-demo/{ring-demo.sh,agent.py}     installed by ring-demo.sh install (root, 0755/0644)
/opt/ring-demo/bin/ring                    ring CLI v1.0.0, aarch64 musl, SHA-256 pinned in ring-demo.sh
/etc/systemd/system/ring-demo.service      the agent
/etc/ring-demo.env                         RING_DEMO_SILICON, written by ring-demo.sh login (root, 0644)
/var/lib/ring-demo                         home of the ring-demo system user (0700): .ring/, agent-state.json
```

The demo has no listener and no port. It changes nothing that belongs to Ring:
`install-release.sh`, Ring's units, Caddy and `/opt/ring` are untouched. A Ring deploy leaves the
demo running, and the agent carries on once ring-server is back.

## Set it up

You need:
- AWS credentials for the `silicon-ring` stack, as for `deploy/deploy.py`;
- `silicon-accounts` on your machine, signed in as yourself (a carbon).

1. **Check that the id is free:**

   ```sh
   silicon-accounts id available si:ring-demo    # exit 0: available; 5: taken
   ```

   If it is taken, choose another id. Use it in place of `si:ring-demo` below, pass it to
   `ship.py login --silicon`, and put it in the review notes.

2. **Create the silicon.** You become its custodian:

   ```sh
   silicon-accounts silicon create --id si:ring-demo --display-name "Ring Demo"
   ```

   It prints the STK once. Keep it in your password manager.

3. **Sign it in on your machine,** in a folder of its own so your own sign-in is untouched:

   ```sh
   mkdir -m 700 ~/.ring-demo
   printf 'STK: '; read -rs STK; echo
   printf '%s' "$STK" | silicon-accounts --home ~/.ring-demo login --silicon si:ring-demo --stk-stdin; unset STK
   silicon-accounts --home ~/.ring-demo login status --json      # "id":"si:ring-demo","kind":"silicon"
   ```

   While that folder is signed out, never run `silicon-accounts --home ~/.ring-demo login` without
   `--silicon`: it would start a browser sign-in for a carbon instead.

4. **Install it on the host:**

   ```sh
   deploy/demo-silicon/ship.py install
   ```

   This uploads the three files, as one archive, to `s3://…/releases/demo-silicon/` and runs
   `ring-demo.sh install` over SSM. That step:
   - creates the `ring-demo` user;
   - downloads the `ring` CLI from Ring's GitHub release and checks it against the pinned SHA-256;
   - installs the agent and its unit.

   Run it again after changing anything in this folder. It restarts the agent if the CLI is
   signed in as the configured silicon.

5. **Sign the host in and start the agent:**

   ```sh
   deploy/demo-silicon/ship.py login                            # or: ship.py login --silicon si:other-id
   ```

   - It checks that `~/.ring-demo` is signed in as the silicon. Then it mints a Ring sign-in
     token from that folder, which is single use and lasts 2 minutes.
   - It puts the token in a private S3 object. The host pipes that object into
     `ring login --slt-stdin` and deletes it. No token appears in an SSM command or a log.
   - The host stops the agent first, signs in, and checks that the CLI is now the silicon. Then
     it writes `/etc/ring-demo.env`. If `ring login` itself fails, the previous session is kept
     and the agent starts again on it.
   - Last, it turns Ting notifications off, sets the voice (`gleam`), voicemail and the declined
     greeting, and starts `ring-demo.service`.

6. **Check it:**

   ```sh
   deploy/demo-silicon/ship.py status   # agent active, runs as si:ring-demo, signed in until …, call backs due, recent log
   ```

The session lasts until Silicon Accounts ends the demo silicon's sign-in to Ring; `status` shows
when. To renew it, run `ship.py login` again.

## Try it

From the TestFlight build, or as any carbon on https://ring.teamofsilicons.com:

1. Call `si:ring-demo`. It answers within a few seconds and talks.
2. Hang up. About 30 s later it calls you back. Lock the iPhone first to see the CallKit screen.
3. Decline that call. A voicemail from Ring Demo appears under Voicemail.
4. Call again within 10 minutes. It answers, but does not call back again.
5. A fourth call within the hour is declined, and the app shows the reason.

Watch what it does with `deploy/deploy.py logs --unit ring-demo --since '15 min ago'`.

## Logs

The agent writes one line per action to the journal. For example:
- `answered ring_… from c:…`, `declined ring_… from si:…: only carbons are answered`
- `asked the representative on ring_… to say goodbye`, `hung up ring_…: it reached 5 minutes`
- `will call c:… back in 30 s`, `not calling c:… back: its representative failed`
- `calling c:… back: ring_…`, `left a voicemail …`

The same error is logged at most once every 5 minutes. The lines name calls and accounts by id.
They never hold a token or what was said.

```sh
deploy/deploy.py logs --unit ring-demo --lines 100
deploy/demo-silicon/ship.py status          # includes the last 15 lines
```

## Stop, start, remove

| Command | Effect |
|---|---|
| `ship.py stop` | Stops the agent and keeps it signed in. Calls to si:ring-demo then ring for 60 s and go to its voicemail |
| `ship.py start` | Starts it again |
| `ship.py logout` | Stops it and revokes the host's Ring session. Sign in again with `ship.py login` |
| `ship.py remove` | Uninstalls it. The session, state and `/etc/ring-demo.env` stay |
| `ship.py remove --purge` | Also signs out, deletes `/var/lib/ring-demo`, `/etc/ring-demo.env` and the `ring-demo` user |

Keep it running until the app is approved, and again for every later review. When it is no longer
needed:
1. Run `ship.py remove --purge`.
2. If you want the account gone too, delete it as its custodian:
   `silicon-accounts silicon delete si:ring-demo`.
3. Run `rm -rf ~/.ring-demo` on your machine.

## Without ship.py

After `ship.py install`, you can also sign the host in from a Session Manager shell (see
`docs/operations.md`):

```sh
# on your machine; the token works once, within 2 minutes
silicon-accounts --home ~/.ring-demo login status --json    # must say "id":"si:ring-demo"
silicon-accounts --home ~/.ring-demo login --app ring -q    # prints the token
# on the host
sudo /opt/ring-demo/ring-demo.sh login --silicon si:ring-demo   # paste the token, press Enter, then Ctrl-D
sudo /opt/ring-demo/ring-demo.sh status
```

## When something is off

| Symptom | What to do |
|---|---|
| `status` says `signed in: no`, or the log says the CLI was signed out | The session ended or was revoked. Run `ship.py login` |
| `status` says the CLI is signed in as another account, or the log says `refusing to run` | Run `ship.py login` (with `--silicon` if the demo silicon is not si:ring-demo) |
| `ship.py login` says the token signed the CLI in as someone else | The token came from the wrong sign-in. Redo step 3, then `ship.py login` |
| `ship.py login` says the folder is not signed in as si:ring-demo | Redo step 3 |
| Calls to si:ring-demo ring out to voicemail | Check `ship.py status`. If the agent is inactive, run `ship.py start`; if it is failing, read the log |
| A reviewer's call is declined with "already answered a few of your calls this hour" | That caller had 3 calls answered in the last hour. Wait, or call from another account |
| `install` fails with `does not match the pinned SHA-256` | A fix release replaced the CLI archive in place. Update `RING_SHA256` in `ring-demo.sh` with the command in its header, then install again |
| `ring login` says the outcome is unknown | The token may be spent. Run `ship.py login` again, which mints a new one |
| A Ring deploy is in progress | Nothing to do. Calls in progress end, as they do for everyone, and the agent carries on afterwards |

## For the runbook

`docs/operations.md` can point here with:

> **Demo silicon (App Review).** `si:ring-demo` answers calls from carbons on the production host
> (`ring-demo.service`, user `ring-demo`, state in `/var/lib/ring-demo`). Set up, sign in, logs and
> stopping it: `deploy/demo-silicon/README.md`. Logs: `deploy/deploy.py logs --unit ring-demo`.
