#!/usr/bin/env node
// Real CLI/server privacy regression. A controlled greeting player pauses before
// microphone/file capture; no audio hardware or external providers are needed.
import assert from 'node:assert/strict';
import { mkdtemp, writeFile, readFile, rm, chmod } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { createServer } from 'node:net';

if (process.platform === 'win32') {
  console.log('The controlled native-player fixture uses a Unix executable; run on macOS/Linux.');
  process.exit(0);
}
const root = await mkdtemp(join(tmpdir(), 'ring-cli-private-'));
const cli = resolve('target/debug/ring'), serverBinary = resolve('target/debug/ring-server');
const listener = createServer(); listener.listen(0, '127.0.0.1'); await once(listener, 'listening');
const port = listener.address().port; await new Promise(resolve => listener.close(resolve));
const secret = 'isolated-private-voicemail-test-secret';
const identities = {
  caller: { actor: 'c:caller', org_id: 'call-org' },
  other: { actor: 'c:caller', org_id: 'other-org' },
  peer: { actor: 'c:peer', org_id: 'peer-org' },
  recipient: { actor: 'c:recipient', org_id: 'recipient-org' },
};
const env = Object.fromEntries(Object.entries(process.env).filter(([key]) => !/^(RING_|SILICON_RING_|SILICON_HOME$|SILICON_ORG$)/.test(key)));
Object.assign(env, { RING_BIND: `127.0.0.1:${port}`, RING_DATA_DIR: join(root, 'server'), RING_TEST_APP_SECRET: secret, RING_TEST_TOKENS_FILE: join(root, 'tokens.json'), RING_DISABLE_PROVIDERS: '1', RING_TELEMETRY_ENABLED: 'false', SILICON_HOME: join(root, 'home'), SILICON_ORG: 'other-org', SILICON_RING_TEST_APP_SECRET: secret, SILICON_RING_SERVER_URL: `ws://127.0.0.1:${port}/ws`, PATH: `${root}:${env.PATH}` });
await writeFile(env.RING_TEST_TOKENS_FILE, JSON.stringify(identities), { mode: 0o600 });
const player = join(root, process.platform === 'darwin' ? 'afplay' : 'aplay');
await writeFile(player, `#!${process.execPath}\nimport('node:fs').then(fs => { fs.writeFileSync(process.env.RING_TEST_PLAYER_READY, String(process.pid)); const timer = setInterval(() => { if (fs.existsSync(process.env.RING_TEST_PLAYER_RELEASE)) { clearInterval(timer); process.exit(Number(fs.readFileSync(process.env.RING_TEST_PLAYER_RELEASE, 'utf8'))); } }, 20); });\n`);
await chmod(player, 0o755);
const pcm = Buffer.alloc(960); for (let i = 0; i < pcm.length; i += 2) pcm.writeInt16LE(1000, i);
const wav = Buffer.alloc(44 + pcm.length);
wav.write('RIFF'); wav.writeUInt32LE(wav.length - 8, 4); wav.write('WAVEfmt ', 8); wav.writeUInt32LE(16, 16); wav.writeUInt16LE(1, 20); wav.writeUInt16LE(1, 22); wav.writeUInt32LE(24000, 24); wav.writeUInt32LE(48000, 28); wav.writeUInt16LE(2, 32); wav.writeUInt16LE(16, 34); wav.write('data', 36); wav.writeUInt32LE(pcm.length, 40); pcm.copy(wav, 44);
await writeFile(join(root, 'message.wav'), wav);
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
async function waitFor(check, label) { for (let i = 0; i < 150; i++) { const value = await check(); if (value) return value; await delay(20); } throw new Error(`Timed out: ${label}`); }
function run(args, extra = {}) {
  const child = spawn(cli, ['--test', '--json', ...args], { env: { ...env, ...extra }, stdio: ['ignore', 'pipe', 'pipe'] });
  let stdout = '', stderr = ''; child.stdout.on('data', b => stdout += b); child.stderr.on('data', b => stderr += b);
  const done = once(child, 'exit').then(([code]) => ({ code, stdout, stderr }));
  return { child, done };
}
class Client {
  constructor(socket) {
    this.socket = socket; this.pending = new Map(); this.frames = [];
    socket.addEventListener('message', ({ data }) => { const v = JSON.parse(data); if (this.pending.has(v.id)) { const { resolve, reject, timer } = this.pending.get(v.id); clearTimeout(timer); this.pending.delete(v.id); v.ok ? resolve(v.result) : reject(Object.assign(new Error(v.error.message), v.error)); } else if (v.type === 'media.audio') this.frames.push(v.data); });
  }
  request(method, params = {}) { const id = crypto.randomUUID(); return new Promise((resolve, reject) => { const timer = setTimeout(() => reject(new Error(`Timeout: ${method}`)), 5000); this.pending.set(id, { resolve, reject, timer }); this.socket.send(JSON.stringify({ id, method, params })); }); }
  frame(type, data) { this.socket.send(JSON.stringify({ type, data })); }
}
let server, feed, foreground; const clients = [];
try {
  server = spawn(serverBinary, [], { env, stdio: 'ignore' });
  await waitFor(async () => { try { return (await fetch(`http://127.0.0.1:${port}/health`)).ok; } catch { return false; } }, 'server');
  async function login(token) {
    const ws = new WebSocket(env.SILICON_RING_SERVER_URL); await once(ws, 'open');
    const c = new Client(ws); clients.push(c);
    await c.request('protocol.hello', { versions: [1], realm: 'test', org_id: identities[token].org_id, test_app_secret: secret });
    c.session = await c.request('auth.login', { token }); return c;
  }
  const caller = await login('caller'), peer = await login('peer'), recipient = await login('recipient');
  assert.equal((await run(['login', 'other']).done).code, 0);
  const active = await caller.request('calls.init', { target: 'c:peer' });
  await peer.request('calls.accept', { ringid: active.ringid });
  const mic = await caller.request('media.attach', { ringid: active.ringid, device_id: caller.session.device_id, purpose: 'call' });
  const ears = await peer.request('media.attach', { ringid: active.ringid, device_id: peer.session.device_id, purpose: 'call' });
  let seq = 0; feed = setInterval(() => { caller.frame('media.audio', { stream_id: mic.stream_id, seq, offset_ms: seq * 20, audio_base64: pcm.toString('base64') }); peer.frame('media.audio', { stream_id: ears.stream_id, seq, offset_ms: seq++ * 20, audio_base64: Buffer.alloc(960).toString('base64') }); }, 20);
  const audible = frames => frames.some(f => Buffer.from(f.audio_base64, 'base64').some(b => b !== 0));
  await waitFor(() => audible(peer.frames), 'baseline conference microphone');
  const greeting = await recipient.request('assets.begin', { purpose: 'voicemail_greeting', mime_type: 'audio/wav', size_bytes: wav.length });
  recipient.frame('assets.chunk', { asset_id: greeting.asset_id, seq: 0, data_base64: wav.toString('base64') });
  await recipient.request('assets.complete', { asset_id: greeting.asset_id });
  await recipient.request('config.set', { scope: 'actor', values: { 'voicemail.greetings.declined': { asset_id: greeting.asset_id } } });
  for (const outcome of ['success', 'failure', 'cancel', 'previously-muted']) {
    if (outcome === 'previously-muted') await caller.request('media.state', { stream_id: mic.stream_id, muted: true });
    const call = await caller.request('calls.init', { target: 'c:recipient' });
    await recipient.request('calls.decline', { ringid: call.ringid });
    const ready = join(root, `${outcome}.ready`), release = join(root, `${outcome}.release`);
    foreground = run(['voicemail', 'leave', call.ringid, '--audio-file', join(root, 'message.wav')], { RING_TEST_PLAYER_READY: ready, RING_TEST_PLAYER_RELEASE: release });
    const playerPid = Number(await waitFor(() => readFile(ready, 'utf8').catch(() => false), 'greeting before capture'));
    await delay(60); const start = peer.frames.length; await delay(220);
    const privateFrames = peer.frames.slice(start); assert(privateFrames.length > 0);
    assert(!audible(privateFrames), 'Another org/device conference microphone must be suppressed BEFORE local capture');
    const status = JSON.parse((await run(['daemon', 'status']).done).stdout); assert.equal(status.private_recordings, 1);
    const switching = await run(['login', 'other']).done;
    assert.equal(JSON.parse(switching.stderr).error.code, 'AUDIO_BUSY', 'Account replacement must not orphan another session’s private reservation');
    if (outcome === 'cancel') foreground.child.kill('SIGINT'); else await writeFile(release, outcome === 'failure' ? '1' : '0');
    const result = await foreground.done; foreground = undefined;
    assert.equal(result.code, outcome === 'failure' ? 5 : outcome === 'cancel' ? 130 : 0, JSON.stringify(result));
    await waitFor(() => { try { process.kill(playerPid, 0); return false; } catch { return true; } }, 'native player cleanup');
    const resumed = peer.frames.length;
    if (outcome === 'previously-muted') {
      await delay(220); assert(!audible(peer.frames.slice(resumed)), 'Cleanup must preserve the conference’s original muted setting');
      await caller.request('media.state', { stream_id: mic.stream_id, muted: false });
    }
    await waitFor(() => audible(peer.frames.slice(resumed)), 'conference microphone restored');
    assert.equal(JSON.parse((await run(['daemon', 'status']).done).stdout).private_recordings, 0);
    console.log(`PASS: cross-org CLI private capture and ${outcome} cleanup`);
  }
} finally {
  foreground?.child.kill('SIGINT'); if (foreground) await foreground.done;
  clearInterval(feed); await run(['daemon', 'stop']).done.catch(() => {});
  for (const client of clients) client.socket.close();
  if (server?.exitCode === null) { const exited = once(server, 'exit'); server.kill('SIGINT'); await exited; }
  await rm(root, { recursive: true, force: true });
}
