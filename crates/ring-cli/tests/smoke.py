#!/usr/bin/env python3
"""Headless CLI/server contract test; no external providers or real credentials.
Run: cargo build -p ring-server -p ring-cli && python3 crates/ring-cli/tests/smoke.py
"""
import base64, faulthandler, json, os, pathlib, socket, subprocess, tempfile, time, urllib.request
faulthandler.dump_traceback_later(60, repeat=True)
ROOT = pathlib.Path(__file__).resolve().parents[3]
BIN = ROOT / ("target/debug/ring.exe" if os.name == "nt" else "target/debug/ring")
SERVER = ROOT / ("target/debug/ring-server.exe" if os.name == "nt" else "target/debug/ring-server")
with tempfile.TemporaryDirectory(prefix="ring-cli-") as temp:
    root = pathlib.Path(temp)
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        port = sock.getsockname()[1]
    tokens = root / "tokens.json"
    tokens.write_text(json.dumps({"alice-test-token": {"actor": "si:alice", "org_id": "test-org", "admin": True}, "bob-test-token": {"actor": "si:bob", "org_id": "test-org"}}))
    tokens.chmod(0o600)
    env = {**os.environ, "RING_BIND": f"127.0.0.1:{port}", "RING_DATA_DIR": str(root / "server"), "RING_TEST_APP_SECRET": "cli-integration-secret-with-32-chars", "RING_TEST_TOKENS_FILE": str(tokens), "RING_DISABLE_PROVIDERS": "1", "RING_TELEMETRY_ENABLED": "false", "SILICON_RING_TEST_APP_SECRET": "cli-integration-secret-with-32-chars", "SILICON_RING_SERVER_URL": f"ws://127.0.0.1:{port}/ws", "SILICON_ORG": "test-org"}
    server_log = (root / "server.log").open("w+")
    server = subprocess.Popen([str(SERVER)], cwd=ROOT, env=env, stdout=server_log, stderr=server_log)
    actors = ["alice", "bob"]
    def run(actor, *args, code=0, stdin=None):
        print("CLI smoke:", actor, *args[:3], flush=True)
        actor_env = {**env, "SILICON_HOME": str(root / actor)}
        if actor == "alice":
            actor_env.pop("SILICON_RING_SERVER_URL")
        result = subprocess.run([str(BIN), "--json", "--test", *args], input=stdin, capture_output=True, text=True, env=actor_env, timeout=30)
        assert result.returncode == code, (args, result.returncode, result.stdout, result.stderr)
        text = result.stdout if code == 0 else result.stderr
        assert "alice-test-token" not in text and "bob-test-token" not in text
        return json.loads(text)
    try:
        for _ in range(100):
            try:
                urllib.request.urlopen(f"http://127.0.0.1:{port}/health", timeout=.1)
                break
            except OSError:
                assert server.poll() is None, "server exited"
                time.sleep(.05)
        # Alice uses her persisted URL; Bob's environment overrides a stale local URL.
        defaults = run("alice", "config", "show", "--scope", "local")
        assert defaults["effective"]["server_url"] == "wss://backend.ring.teamofsilicons.com/ws"
        assert defaults["origins"]["server_url"] == "default"
        run("alice", "config", "set", "--scope", "local", json.dumps({"server_url": env["SILICON_RING_SERVER_URL"]}))
        run("bob", "config", "set", "--scope", "local", '{"server_url":"ws://127.0.0.1:1/ws"}')
        local = run("alice", "config", "show", "--scope", "local")
        overridden = run("bob", "config", "show", "--scope", "local")
        assert local["effective"]["server_url"] == env["SILICON_RING_SERVER_URL"]
        assert local["origins"]["server_url"] == "local"
        assert overridden["values"]["server_url"] == "ws://127.0.0.1:1/ws"
        assert overridden["effective"]["server_url"] == env["SILICON_RING_SERVER_URL"]
        assert overridden["origins"]["server_url"] == "SILICON_RING_SERVER_URL"
        assert overridden["effective"]["audio.input"] is None
        assert overridden["origins"]["audio.input"] == "default"
        assert run("alice", "login", "status")["authenticated"] is False
        for actor in actors:
            result = run(actor, "login", "--token-stdin", stdin=f"{actor}-test-token\n")
            assert result["actor"] == f"si:{actor}"
            session = next((root / actor / ".ring").glob("*-session.json"))
            if os.name != "nt":
                assert session.stat().st_mode & 0o777 == 0o600
        photo=root/'avatar.png'
        photo.write_bytes(base64.b64decode('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+/l9sAAAAASUVORK5CYII='))
        uploaded=run('alice','--request-id','photo-upload','profile','set','--photo',str(photo))
        retry=run('alice','--request-id','photo-upload','profile','set','--photo',str(photo))
        assert uploaded['photo_asset_id']==retry['photo_asset_id']
        run("alice", "config", "set", '{"representative.default_context":"Use brief answers."}')
        denied = run("alice", "--request-id", "approved-call", "call", "init", "si:bob", "--context", "Discuss launch.", code=4)
        assert denied["error"]["code"] == "CONTEXT_APPROVAL_REQUIRED"
        assert len(run("alice", "call", "history")["items"]) == 0
        preview = denied["error"]["details"]
        run("alice", "context", "approve", preview["preparation_id"], "--for", "1h")
        first = run("alice", "--request-id", "approved-call", "call", "init", "si:bob", "--context", "Discuss launch.")
        retry = run("alice", "--request-id", "approved-call", "call", "init", "si:bob[another-org]", "--context", "Discuss launch.")
        assert first["ringid"] == retry["ringid"]
        ring = first["ringid"]
        accepted = run("bob", "call", "accept", ring, "--start", "Hello")
        assert accepted["state"] == "active"
        run("alice", "send", ring, "thinking", "Private reasoning")
        run("alice", "send", ring, "commentary", "The launch is ready.")
        bob_transcript = run("bob", "call", "transcript", ring)
        assert "Private reasoning" not in json.dumps(bob_transcript)
        run("alice", "send", ring, "thinking", "🦀" * 161, code=2)
        run("alice", "daemon", "stop")
        assert run("alice", "call", "show", ring)["state"] == "active"
        run("alice", "call", "cut", ring)
        assert run("bob", "call", "show", ring)["state"] == "ended"
        recording=root/'recording.wav'
        for _ in range(100):
            if run('alice','call','recording',ring)['status']=='ready': break
            time.sleep(.05)
        run('alice','call','recording',ring,'--audio-out',str(recording))
        assert recording.read_bytes()[:4]==b'RIFF'
        run('alice','call','recording',ring,'--audio-out',str(recording),code=4)
        run('alice','call','recording',ring,'--audio-out',str(recording),'--overwrite')
        # A second login must open a fresh connection, never reuse the old authenticated peer.
        run("alice", "login", "--token-stdin", stdin="alice-test-token\n")
        run("alice", "logout")
        assert run("alice", "login", "status")["authenticated"] is False
        print("CLI/server smoke: authentication, privacy, approval, idempotency, lifecycle, daemon restart and validation passed", flush=True)
    finally:
        print("CLI smoke: cleanup", flush=True)
        for actor in actors:
            subprocess.run([str(BIN), "--test", "daemon", "stop"], env={**env, "SILICON_HOME": str(root / actor)}, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=10)
        server.terminate()
        server.wait(timeout=10)
        server_log.close()

faulthandler.cancel_dump_traceback_later()
