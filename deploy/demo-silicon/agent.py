#!/usr/bin/env python3
"""The App Review demo silicon (si:ring-demo, "Ring Demo").

Signed in to the ring CLI as the demo silicon (and refusing to run as anyone else), it:

- answers calls from carbons (people), giving its voice representative a short demo context;
- declines, with a short reason, a call from a silicon, a call with no carbon on it, and a
  carbon's call once it has answered 3 of theirs in the last hour;
- answers any delegation at once, so the representative never waits for it;
- tells its representative to say goodbye 30 s before a call reaches 5 minutes, then hangs up;
- hangs up at once when its representative fails, or when no carbon is left on the call;
- when a call it answered from a carbon ends normally, calls that carbon back once, 30 s later,
  at most once per caller every 10 minutes and never while it is on a call;
- leaves a short voicemail when that call back is declined or missed.

It does nothing else. It polls the Ring API through the ring CLI every 2 s, so it needs no
listener, webhook, Ting daemon or gateway change. What must survive a restart (the call backs
that are due, when each caller was last called back, and the calls it answered in the last hour)
is kept in --state; everything else is read back from the API, including a call that was in
progress when the agent restarted.

    agent.py --ring /opt/ring-demo/bin/ring --silicon si:ring-demo --state /var/lib/ring-demo/agent-state.json

Exit status 78 means the CLI is signed in as some other account; it never runs that way.

Python 3.9 or newer, standard library only. Logs go to stderr (the journal) and name calls and
accounts by id; they never hold a token or what was said.
"""

from __future__ import annotations

import argparse
import datetime
import json
import logging
import os
import pathlib
import re
import signal
import subprocess
import sys
import threading
import time
from typing import Any, Dict, List, Optional, Tuple

# Text limits (udd/cli.md): context 400, start 100, send 160, decline reason 200, voicemail 600
# code points. {minutes} is the call length cap (--max-call).
CONTEXT = (
    "You are Ring Demo, a demo silicon that answers calls from people trying the Ring app. Ring is "
    "a phone app where carbons (people) and silicons (AI agents) call each other by id, like "
    "c:alice or si:ring-demo. Calls are recorded and transcribed, and end after {minutes} minutes. "
    "Be friendly and brief. Answer simple questions about Ring yourself. Never ask your silicon for "
    "help: you can't look anything up."
)
START = "Hi, this is Ring Demo. Ask me anything about Ring."
CALLBACK_CONTEXT = (
    "You are Ring Demo, a demo silicon, calling back someone who just tried the Ring app so they "
    "can see an incoming call. Ring is a phone app where carbons (people) and silicons (AI agents) "
    "call each other by id. Calls are recorded and transcribed, and end after {minutes} minutes. Be "
    "friendly and brief. Answer simple questions about Ring yourself. Never ask your silicon for "
    "help: you can't look anything up."
)
CALLBACK_START = "Hi, it's Ring Demo calling you back, so you can see an incoming call."
VOICEMAIL = (
    "Hi, it's Ring Demo. Sorry I missed you. I called back so you could try an incoming call and "
    "a voicemail in Ring. There's nothing you need to do. Bye!"
)
DELEGATION_REPLY = "This is a demo, so I can't look that up. Ask me about Ring instead."
WRAP_UP = "Ring Demo calls last {minutes} minutes, so this one ends in about 30 seconds. Say a short goodbye now."
DECLINE_SILICON = "Ring Demo only talks with people (carbons). It is a demo for people trying the Ring app."
DECLINE_NO_CARBON = "Ring Demo only talks with people (carbons), and no person is on this call."
DECLINE_LIMIT = "Ring Demo has already answered a few of your calls this hour. Please try again later."

LIVE_STATES = "ringing,active,voicemail"
SIGNED_OUT_WAIT = 60.0
GIVE_UP_FOLLOWING = 120.0
WRAP_UP_LEAD = 30.0
ANSWER_WINDOW = 3600.0
ERROR_REPEAT = 300.0
MAX_TRIES = 3
EXIT_WRONG_ACCOUNT = 78  # EX_CONFIG; ring-demo.service does not restart on it
SILICON_ID = re.compile(r"^si:[a-z0-9_-]{3,30}$")
# Exit 3 means "not signed in or not allowed"; only these codes mean the session itself is gone.
SIGNED_OUT_CODES = {"not_signed_in", "unauthenticated", "session_expired", "session_api_mismatch"}
# Exit codes worth retrying: failed or network (1), rate limited (6), interrupted (130).
RETRY_EXITS = {1, 6, 130}

log = logging.getLogger("ring-demo")


class RingError(Exception):
    def __init__(self, code: str, message: str, exit_code: int, retryable: bool = False):
        super().__init__(f"{code}: {message}")
        self.code = code
        self.message = message
        self.exit = exit_code
        self.retryable = retryable

    @property
    def signed_out(self) -> bool:
        return self.exit == 3 and self.code in SIGNED_OUT_CODES

    @property
    def transient(self) -> bool:
        """Worth trying again later: a network error, a timeout, a rate limit, or a command cut
        off by a signal. Anything else (not found, invalid, conflict) will fail the same way."""
        return self.retryable or self.exit in RETRY_EXITS


class WrongAccount(Exception):
    pass


class Ring:
    """Runs one ring CLI command with --json and returns what it printed, parsed."""

    def __init__(self, path: str, timeout: float = 30.0):
        self.path = path
        self.timeout = timeout

    def __call__(self, *args: str, key: Optional[str] = None) -> Any:
        command = [self.path, "--json"]
        if key:
            command += ["--idempotency-key", key]
        command += list(args)
        name = " ".join(args[:2])
        try:
            # A session of its own: a Ctrl-C or SIGTERM meant for the agent never cuts a command
            # off halfway. The agent stops between commands instead.
            done = subprocess.run(
                command,
                stdin=subprocess.DEVNULL,
                capture_output=True,
                text=True,
                timeout=self.timeout,
                start_new_session=True,
            )
        except subprocess.TimeoutExpired:
            raise RingError("timeout", f"ring {name} took longer than {self.timeout:.0f} s", 1, True)
        except OSError as error:
            raise RingError("spawn_failed", f"could not run {self.path}: {error.strerror}", 1)
        if done.returncode < 0:
            raise RingError("interrupted", f"ring {name} was stopped by signal {-done.returncode}", 130, True)
        if done.returncode != 0:
            code, message, retryable = "failed", f"ring {name} exited {done.returncode}", False
            for line in reversed(done.stderr.strip().splitlines()):
                if line.startswith("{"):
                    try:
                        error = json.loads(line)["error"]
                        code = error.get("code") or code
                        message = error.get("message") or message
                        retryable = error.get("retryable") is True
                    except (ValueError, KeyError, TypeError, AttributeError):
                        pass
                    break
            raise RingError(code, message, done.returncode, retryable)
        try:
            return json.loads(done.stdout) if done.stdout.strip() else {}
        except ValueError:
            raise RingError("bad_output", f"ring {name} printed something other than JSON", 1)


def timestamp(value: Any) -> Optional[float]:
    """Seconds since the epoch for an RFC 3339 time from the API (2026-10-15T09:30:00.000Z)."""
    if not isinstance(value, str) or not value:
        return None
    try:
        return datetime.datetime.fromisoformat(value.replace("Z", "+00:00")).timestamp()
    except ValueError:
        return None


class Agent:
    def __init__(self, ring: Ring, silicon: str, state_path: pathlib.Path, poll: float, callback_delay: float,
                 callback_every: float, give_up: float, max_call: float, per_hour: int):
        self.ring = ring
        self.silicon = silicon
        self.state_path = state_path
        self.poll = poll
        self.callback_delay = callback_delay
        self.callback_every = callback_every
        self.give_up = give_up
        self.max_call = max_call
        self.per_hour = per_hour
        minutes = f"{max_call / 60:g}"
        self.context = CONTEXT.format(minutes=minutes)
        self.callback_context = CALLBACK_CONTEXT.format(minutes=minutes)
        self.wrap_up_text = WRAP_UP.format(minutes=minutes)
        self.stop = threading.Event()
        self.exit_code = 0
        self.me: Optional[Dict[str, Any]] = None
        self.current: Optional[Dict[str, Any]] = None
        self.failed_invitations: Dict[str, int] = {}
        self.signed_out_logged = False
        self.last_error = ""
        self.last_error_at = 0.0
        self.error_repeats = 0
        self.state = self.load_state()

    # ---- state that survives a restart ----------------------------------------------------------

    def load_state(self) -> Dict[str, Any]:
        try:
            data = json.loads(self.state_path.read_text())
            if not isinstance(data, dict):
                raise ValueError("not a JSON object")
        except FileNotFoundError:
            data = {}
        except (OSError, ValueError) as error:
            log.warning("could not read %s (%s); starting with no call backs due", self.state_path, error)
            data = {}
        # Only carbons are ever called back; an entry from an older agent without a kind is a
        # carbon only if its id says so.
        pending = [p for p in data.get("pending") or []
                   if isinstance(p, dict) and p.get("uuid")
                   and (p.get("kind") == "carbon" or (p.get("kind") is None and str(p.get("id", "")).startswith("c:")))]
        last = {k: float(v) for k, v in (data.get("last") or {}).items() if isinstance(v, (int, float))}
        answered = {k: [float(t) for t in v if isinstance(t, (int, float))]
                    for k, v in (data.get("answered") or {}).items() if isinstance(v, list)}
        return {"pending": pending, "last": last, "answered": answered}

    def save_state(self) -> None:
        now = time.time()
        self.state["last"] = {k: v for k, v in self.state["last"].items() if v > now - self.callback_every}
        answered = {k: [t for t in v if t > now - ANSWER_WINDOW] for k, v in self.state["answered"].items()}
        self.state["answered"] = {k: v for k, v in answered.items() if v}
        temporary = self.state_path.with_name(self.state_path.name + ".tmp")
        try:
            temporary.write_text(json.dumps(self.state, sort_keys=True))
            os.replace(temporary, self.state_path)
        except OSError as error:
            log.error("could not save %s: %s", self.state_path, error)

    # ---- main loop -----------------------------------------------------------------------------

    def run(self) -> int:
        log.info("starting as %s: poll every %.1f s, calls end at %.0f min, %d answered calls per caller "
                 "per hour, call back after %.0f s, at most once per caller every %.0f min",
                 self.silicon, self.poll, self.max_call / 60, self.per_hour, self.callback_delay,
                 self.callback_every / 60)
        while not self.stop.is_set():
            wait = self.poll
            try:
                if not self.signed_in():
                    wait = SIGNED_OUT_WAIT
                elif self.current:
                    self.follow()
                else:
                    self.idle()
            except WrongAccount as error:
                log.error("%s; refusing to run. Sign in as %s with ship.py login", error, self.silicon)
                self.exit_code = EXIT_WRONG_ACCOUNT
                break
            except RingError as error:
                if error.signed_out:
                    if not self.signed_out_logged:
                        log.warning("the ring CLI was signed out (%s); waiting for ship.py login", error.code)
                        self.signed_out_logged = True
                    self.me = None
                    wait = SIGNED_OUT_WAIT
                else:
                    self.report(f"ring: {error}")
            except Exception as error:  # noqa: BLE001 - one bad poll must not stop the demo
                self.report(f"unexpected error: {type(error).__name__}: {error}", trace=True)
            self.stop.wait(wait)
        log.info("stopped")
        return self.exit_code

    def report(self, text: str, trace: bool = False) -> None:
        """Logs an error, but the same one at most once every 5 minutes."""
        now = time.time()
        if text == self.last_error and now - self.last_error_at < ERROR_REPEAT:
            self.error_repeats += 1
            return
        again = ""
        if text == self.last_error and self.error_repeats:
            again = f" ({self.error_repeats} more times since it was last logged)"
        log.warning("%s (retrying)%s", text, again, exc_info=trace)
        self.last_error, self.last_error_at, self.error_repeats = text, now, 0

    def signed_in(self) -> bool:
        if self.me:
            return True
        status = self.ring("login", "status")
        if not status.get("authenticated"):
            if not self.signed_out_logged:
                log.warning("the ring CLI is not signed in; run ship.py login")
                self.signed_out_logged = True
            return False
        if status.get("id") != self.silicon or status.get("kind") != "silicon":
            raise WrongAccount(f"the ring CLI is signed in as {status.get('id') or 'an unknown account'}, "
                               f"not {self.silicon}")
        self.me = {"uuid": status.get("uuid"), "id": status.get("id")}
        self.signed_out_logged = False
        log.info("signed in as %s; answering calls", self.me["id"])
        return True

    # ---- not on a call ----------------------------------------------------------------------------

    def idle(self) -> None:
        page = self.ring("call", "history", "--state", LIVE_STATES, "--limit", "20")
        calls = sorted(page.get("items") or [], key=lambda c: c.get("created_at") or "")
        for call in calls:
            invitation = self.ringing_invitation(call)
            if not invitation:
                continue
            refusal = self.screen(call, invitation)
            if refusal:
                self.decline(call, invitation, *refusal)
            elif self.answer(call, invitation):
                return
        # A call this agent is on but not following: it was placed or answered before a restart.
        for call in calls:
            if self.joined(call):
                role = "answered" if call.get("direction") == "incoming" else "callback"
                log.info("following %s, which was in progress", call.get("ringid"))
                self.follow_call(call["ringid"], role, self.peer_of(call),
                                 timestamp(call.get("answered_at")) or time.time())
                return
        self.call_back_if_due()

    def screen(self, call: Dict[str, Any], invitation: Dict[str, Any]) -> Optional[Tuple[str, str]]:
        """Why this call must not be answered, as (the reason the caller sees, what to log), or
        None to answer it."""
        caller = self.caller_of(call, invitation)
        if caller.get("kind") != "carbon":
            return DECLINE_SILICON, "only carbons are answered"
        if not self.carbon_on(call):
            return DECLINE_NO_CARBON, "no carbon is on the call"
        answered = len(self.answered_recently(caller.get("uuid")))
        if answered >= self.per_hour:
            return DECLINE_LIMIT, f"already answered {answered} of their calls in the last hour"
        return None

    def answer(self, call: Dict[str, Any], invitation: Dict[str, Any]) -> bool:
        ringid, invitation_id = call["ringid"], invitation.get("invitation_id") or ""
        if self.failed_invitations.get(invitation_id, 0) >= MAX_TRIES:
            return False
        caller = self.caller_of(call, invitation)
        which = ["--invitation", invitation_id] if invitation_id else []
        try:
            self.ring("call", "accept", ringid, *which, "--context", self.context, "--context-mode", "overwrite",
                      "--start", START)
        except RingError as error:
            if error.signed_out:
                raise
            self.failed(invitation_id)
            log.warning("could not answer %s from %s: %s", ringid, caller.get("id"), error)
            return False
        now = time.time()
        if caller.get("uuid"):
            self.state["answered"].setdefault(caller["uuid"], []).append(now)
            self.save_state()
        log.info("answered %s from %s", ringid, caller.get("id"))
        self.follow_call(ringid, "answered", caller, now)
        return True

    def decline(self, call: Dict[str, Any], invitation: Dict[str, Any], reason: str, why: str) -> None:
        ringid, invitation_id = call["ringid"], invitation.get("invitation_id") or ""
        if self.failed_invitations.get(invitation_id, 0) >= MAX_TRIES:
            return
        caller = self.caller_of(call, invitation)
        which = ["--invitation", invitation_id] if invitation_id else []
        try:
            self.ring("call", "decline", ringid, *which, "--reason", reason)
        except RingError as error:
            if error.signed_out:
                raise
            self.failed(invitation_id)
            log.warning("could not decline %s from %s (%s): %s", ringid, caller.get("id"), why, error)
            return
        log.info("declined %s from %s: %s", ringid, caller.get("id"), why)

    def failed(self, invitation_id: str) -> None:
        if len(self.failed_invitations) > 1000:
            self.failed_invitations.clear()
        self.failed_invitations[invitation_id] = self.failed_invitations.get(invitation_id, 0) + 1

    def answered_recently(self, uuid: Optional[str]) -> List[float]:
        cutoff = time.time() - ANSWER_WINDOW
        return [t for t in self.state["answered"].get(uuid or "", []) if t > cutoff]

    def call_back_if_due(self) -> None:
        now = time.time()
        for entry in sorted(self.state["pending"], key=lambda p: p.get("due", 0)):
            if self.stop.is_set():
                return
            if entry.get("until", 0) < now:
                log.info("gave up calling %s back: Ring Demo stayed busy", entry.get("id"))
                self.drop(entry)
                continue
            if entry.get("due", 0) > now:
                continue
            try:
                created = self.ring("call", "init", entry["uuid"], "--context", self.callback_context,
                                    "--context-mode", "overwrite", "--start", CALLBACK_START,
                                    key=f"ring-demo-callback-{entry.get('source', '')}")
            except RingError as error:
                if error.signed_out:
                    raise
                if error.code == "already_in_call":
                    entry["due"] = now + 10
                    self.save_state()
                    return
                if error.transient:
                    # Kept (and saved) for the next try; the same idempotency key makes a call
                    # that did start on the server come back instead of ringing twice.
                    log.warning("could not call %s back yet: %s", entry.get("id"), error)
                    entry["due"] = now + 15
                    self.save_state()
                    return
                log.warning("not calling %s back: %s", entry.get("id"), error)
                self.drop(entry)
                continue
            self.drop(entry)
            ringid = (created.get("call") or {}).get("ringid")
            if not ringid:
                log.warning("ring call init printed no ringid; not following the call back")
                return
            log.info("calling %s back: %s", entry.get("id"), ringid)
            self.follow_call(ringid, "callback", {"id": entry.get("id"), "uuid": entry["uuid"], "kind": "carbon"},
                             time.time())
            return

    def drop(self, entry: Dict[str, Any]) -> None:
        self.state["pending"] = [p for p in self.state["pending"] if p is not entry]
        self.save_state()

    # ---- on a call ----------------------------------------------------------------------------------

    def follow_call(self, ringid: str, role: str, peer: Dict[str, Any], since: float) -> None:
        self.current = {"ringid": ringid, "role": role, "peer": peer or {}, "since": since,
                        "cut_at": None, "no_callback": None, "wrapped_up": False,
                        "voicemail_left": False, "voicemail_tries": 0, "replied": set(), "reply_tries": {}}

    def follow(self) -> None:
        current = self.current
        assert current is not None
        ringid = current["ringid"]
        try:
            call = self.ring("call", "show", ringid)
        except RingError as error:
            if error.exit == 4 or error.code == "not_participant":
                log.info("%s is gone (%s)", ringid, error.code)
                self.current = None
                return
            raise
        state = call.get("state")
        me = self.participant(call) or {}
        if state == "ended" or me.get("state") == "left":
            self.ended(call)
            return
        if current["cut_at"]:
            if time.time() - current["cut_at"] > GIVE_UP_FOLLOWING:
                log.warning("%s has not ended %.0f s after hanging up; no longer following it", ringid,
                            GIVE_UP_FOLLOWING)
                self.current = None
            return
        representative = me.get("representative") or {}
        if state in ("ringing", "active") and representative.get("status") == "failed":
            current["no_callback"] = "its representative failed"
            self.hang_up(ringid, "its representative failed")
            return
        if state == "active" and not self.carbon_on(call):
            current["no_callback"] = "no carbon was left on the call"
            self.hang_up(ringid, "no carbon is left on it")
            return
        if state == "active":
            self.answer_delegations(ringid)
        if state == "voicemail" and current["role"] == "callback" and not current["voicemail_left"]:
            self.leave_voicemail(ringid)
        # Talking since the later of the call being answered and Ring Demo joining it (it may have
        # been invited to a call that was already going).
        started = [t for t in (timestamp(call.get("answered_at")), timestamp(me.get("joined_at"))) if t]
        talked = time.time() - (max(started) if started else current["since"])
        if talked >= self.max_call:
            self.hang_up(ringid, f"it reached {self.max_call / 60:g} minutes")
        elif (state == "active" and not current["wrapped_up"] and self.max_call > 2 * WRAP_UP_LEAD
              and talked >= self.max_call - WRAP_UP_LEAD):
            self.wrap_up(ringid)

    def ended(self, call: Dict[str, Any]) -> None:
        current = self.current
        assert current is not None
        self.current = None
        ringid = call.get("ringid") or current["ringid"]
        log.info("%s ended (%s)", ringid, call.get("end_reason") or "Ring Demo left")
        if current["role"] != "answered" or not call.get("answered_at"):
            return
        representative = (self.participant(call) or {}).get("representative") or {}
        if representative.get("status") == "failed" or call.get("end_reason") == "failed":
            current["no_callback"] = current["no_callback"] or "its representative failed"
        if current["no_callback"]:
            log.info("not calling %s back: %s", current["peer"].get("id"), current["no_callback"])
            return
        self.schedule_callback(current["peer"], ringid)

    def hang_up(self, ringid: str, why: str) -> None:
        current = self.current
        assert current is not None
        current["cut_at"] = time.time()
        try:
            self.ring("call", "cut", ringid)
            log.info("hung up %s: %s", ringid, why)
        except RingError as error:
            if error.signed_out:
                raise
            log.warning("could not hang up %s (%s): %s", ringid, why, error)

    def wrap_up(self, ringid: str) -> None:
        current = self.current
        assert current is not None
        current["wrapped_up"] = True
        try:
            self.ring("send", ringid, "commentary", self.wrap_up_text, key=f"ring-demo-wrapup-{ringid}")
            log.info("asked the representative on %s to say goodbye", ringid)
        except RingError as error:
            if error.signed_out:
                raise
            log.warning("could not ask the representative on %s to say goodbye: %s", ringid, error)

    def answer_delegations(self, ringid: str) -> None:
        current = self.current
        assert current is not None
        page = self.ring("delegation", "ls", ringid, "--status", "open")
        for delegation in page.get("items") or []:
            delegation_id = delegation.get("delegation_id")
            if not delegation_id or delegation_id in current["replied"] or delegation.get("replies"):
                continue
            tries = current["reply_tries"].get(delegation_id, 0)
            if tries >= MAX_TRIES:
                continue
            current["reply_tries"][delegation_id] = tries + 1
            try:
                self.ring("send", ringid, "commentary", DELEGATION_REPLY, "--delegation-id", delegation_id,
                          key=f"ring-demo-reply-{delegation_id}")
            except RingError as error:
                if error.signed_out:
                    raise
                log.warning("could not answer delegation %s on %s: %s", delegation_id, ringid, error)
                continue
            current["replied"].add(delegation_id)
            log.info("answered delegation %s on %s", delegation_id, ringid)

    def leave_voicemail(self, ringid: str) -> None:
        current = self.current
        assert current is not None
        current["voicemail_tries"] += 1
        try:
            self.ring("voicemail", "leave", ringid, "--text", VOICEMAIL, key=f"ring-demo-voicemail-{ringid}")
        except RingError as error:
            if error.signed_out:
                raise
            log.warning("could not leave a voicemail on %s: %s", ringid, error)
            current["voicemail_left"] = current["voicemail_tries"] >= MAX_TRIES
            return
        current["voicemail_left"] = True
        log.info("left a voicemail for %s on %s", current["peer"].get("id"), ringid)

    # ---- call backs ---------------------------------------------------------------------------------

    def schedule_callback(self, peer: Dict[str, Any], source: str) -> None:
        if peer.get("kind") != "carbon" or not peer.get("uuid"):
            log.info("not calling %s back: only carbons get a call back", peer.get("id"))
            return
        now = time.time()
        for entry in self.state["pending"]:
            if entry.get("uuid") == peer["uuid"]:
                # They called again before the call back: wait until this call is over too.
                entry["due"] = max(entry.get("due", 0), now + self.callback_delay)
                self.save_state()
                return
        last = self.state["last"].get(peer["uuid"])
        if last and now - last < self.callback_every:
            log.info("not calling %s back: the last call back was %.0f min ago", peer.get("id"), (now - last) / 60)
            return
        self.state["last"][peer["uuid"]] = now
        self.state["pending"].append({"id": peer.get("id"), "uuid": peer["uuid"], "kind": "carbon", "source": source,
                                      "due": now + self.callback_delay, "until": now + self.give_up})
        self.save_state()
        log.info("will call %s back in %.0f s", peer.get("id"), self.callback_delay)

    # ---- reading calls ------------------------------------------------------------------------------

    def is_me(self, account: Optional[Dict[str, Any]]) -> bool:
        return bool(account) and self.me is not None and account.get("uuid") == self.me.get("uuid")

    def ringing_invitation(self, call: Dict[str, Any]) -> Optional[Dict[str, Any]]:
        for invitation in call.get("invitations") or []:
            if self.is_me(invitation.get("to")) and invitation.get("state") == "ringing":
                return invitation
        return None

    def caller_of(self, call: Dict[str, Any], invitation: Dict[str, Any]) -> Dict[str, Any]:
        return invitation.get("from") or call.get("created_by") or {}

    def participant(self, call: Dict[str, Any]) -> Optional[Dict[str, Any]]:
        for participant in call.get("participants") or []:
            if self.is_me(participant.get("account")):
                return participant
        return None

    def joined(self, call: Dict[str, Any]) -> bool:
        participant = self.participant(call)
        return bool(participant) and participant.get("state") == "joined"

    def carbon_on(self, call: Dict[str, Any]) -> bool:
        """Whether a carbon other than Ring Demo is on the call, so two silicons never talk alone."""
        for participant in call.get("participants") or []:
            account = participant.get("account") or {}
            if participant.get("state") == "joined" and account.get("kind") == "carbon" and not self.is_me(account):
                return True
        return False

    def peer_of(self, call: Dict[str, Any]) -> Dict[str, Any]:
        invitations: List[Dict[str, Any]] = call.get("invitations") or []
        if call.get("direction") == "incoming":
            for invitation in invitations:
                if self.is_me(invitation.get("to")):
                    return invitation.get("from") or {}
            return call.get("created_by") or {}
        for invitation in invitations:
            if not self.is_me(invitation.get("to")):
                return invitation.get("to") or {}
        return {}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--ring", default="ring", help="the ring CLI, signed in as the demo silicon")
    parser.add_argument("--silicon", default="si:ring-demo",
                        help="the only account the CLI may be signed in as (default si:ring-demo)")
    parser.add_argument("--state", type=pathlib.Path, required=True, help="where call backs that are due are kept")
    parser.add_argument("--poll", type=float, default=2.0, help="seconds between polls (default 2)")
    parser.add_argument("--max-call", type=float, default=300.0,
                        help="hang up a call after this many seconds (default 300)")
    parser.add_argument("--per-hour", type=int, default=3,
                        help="answer at most this many calls per caller in an hour (default 3)")
    parser.add_argument("--callback-delay", type=float, default=30.0,
                        help="seconds after a call ends before calling back (default 30)")
    parser.add_argument("--callback-every", type=float, default=600.0,
                        help="at most one call back per caller in this many seconds (default 600)")
    parser.add_argument("--give-up", type=float, default=600.0,
                        help="drop a call back still waiting after this many seconds (default 600)")
    args = parser.parse_args()
    logging.basicConfig(level=logging.INFO, format="%(levelname)s %(message)s", stream=sys.stderr)

    if not SILICON_ID.match(args.silicon):
        log.error("--silicon %r is not a silicon id (si: and 3 to 30 of a-z, 0-9, - and _); refusing to run",
                  args.silicon)
        return EXIT_WRONG_ACCOUNT
    agent = Agent(Ring(args.ring), args.silicon, args.state, max(args.poll, 0.2), args.callback_delay,
                  args.callback_every, args.give_up, max(args.max_call, 10.0), max(args.per_hour, 1))
    for name, text, limit in (("context", agent.context, 400), ("callback context", agent.callback_context, 400),
                              ("start", START, 100), ("callback start", CALLBACK_START, 100),
                              ("wrap-up", agent.wrap_up_text, 160), ("delegation reply", DELEGATION_REPLY, 160),
                              ("voicemail", VOICEMAIL, 600), ("silicon decline", DECLINE_SILICON, 200),
                              ("no-carbon decline", DECLINE_NO_CARBON, 200), ("limit decline", DECLINE_LIMIT, 200)):
        if len(text) > limit:
            log.error("the %s text is %d characters; Ring takes at most %d", name, len(text), limit)
            return 2

    def stop(signum: int, _frame: Any) -> None:
        agent.stop.set()

    signal.signal(signal.SIGTERM, stop)
    signal.signal(signal.SIGINT, stop)
    return agent.run()


if __name__ == "__main__":
    sys.exit(main())
