#!/usr/bin/env node
// Real lifecycle HTTP + WebSocket isolation against a loopback-only IAM service.
// Run after: cargo build --locked -p ring-server (Node 22+; no external services).
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { mkdtemp, rm, writeFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import { createServer as createPortReservation } from 'node:net';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';

const directory = await mkdtemp(join(tmpdir(), 'ring-managed-'));
const A = '00000000-0000-4000-8000-000000000001';
const B = '00000000-0000-4000-8000-000000000002';
const controlToken = 'isolated-lifecycle-control-token-32-or-more';
const legacySecret = 'isolated-legacy-ring-app-secret';
const contexts = new Map([
  [A, { key: 'A'.repeat(32), secret: 'ring-app-a' }],
  [B, { key: 'B'.repeat(32), secret: 'ring-app-b' }],
  ['production', { secret: 'ring-app-production' }],
]);
const actors = { alice: { actor: 'c:alice', org: 'alice-org' }, bob: { actor: 'c:bob', org: 'bob-org' } };
const clients = [], timers = [], tokens = new Map(), iamRequests = [], unexpected = [];
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
let server, serverOutput = '', blockedIntrospection, checks = 0;
function checked(message) { console.log(`ok ${++checks} - ${message}`); }
async function waitFor(check, label) {
  for (let n = 0; n < 150; n++) { const value = await check(); if (value) return value; await delay(20); }
  throw new Error(`Timed out: ${label}`);
}
function reply(response, status, body) { response.writeHead(status, { 'content-type': 'application/json' }); response.end(JSON.stringify(body)); }
function reject(response) { reply(response, 401, { error: { code: 'unauthenticated', message: 'Invalid isolated fixture credential' } }); }
function appContext(request) {
  return [...contexts].find(([, context]) => request.headers.authorization === `Basic ${Buffer.from(`ring:${context.secret}`).toString('base64')}` && request.headers['x-testing-environment-key'] === context.key)?.[0];
}
function contextReply(realm) {
  return { environment_id: realm, application: { app_id: 'ring', base_url: 'https://ring.example', app_scope: { iam: [], external: [] }, webhook_scope: [], testing_idle_days: 7 } };
}
const iam = createServer(async (request, response) => {
  try {
    const chunks = []; for await (const chunk of request) chunks.push(chunk);
    const body = Object.fromEntries(new URLSearchParams(Buffer.concat(chunks).toString()));
    const path = new URL(request.url, 'http://localhost').pathname;
    const realm = appContext(request);
    iamRequests.push({ path, realm: realm ?? null });
    if (path === '/api/v1/application/testing-context' && request.method === 'GET') {
      // Valid credentials with a deliberately wrong IAM environment acknowledgement.
      if (request.headers.authorization === `Basic ${Buffer.from('ring:wrong-ack').toString('base64')}` && request.headers['x-testing-environment-key'] === contexts.get(A).key) return reply(response, 200, contextReply(B));
      return realm && realm !== 'production' ? reply(response, 200, contextReply(realm)) : reject(response);
    }
    if (path === '/api/v1/app-auth/tokens' && request.method === 'POST') {
      const [selected, who] = (body.slt ?? '').split('|');
      if (!realm || selected !== realm || !actors[who] || body.app_id !== 'ring' || !request.headers['idempotency-key']) return reject(response);
      const token = `fixture-access-${crypto.randomUUID()}`;
      tokens.set(token, { realm, ...actors[who] });
      return reply(response, 200, { access_token: token, refresh_token: `fixture-refresh-${crypto.randomUUID()}`, token_type: 'Bearer', expires_in: 3600, scope: '' });
    }
    if (path === '/api/v1/oauth/introspect' && request.method === 'POST') {
      const token = tokens.get(body.token);
      if (!realm || token?.realm !== realm || token.org !== request.headers['x-org-id']) return reply(response, 200, { active: false });
      const result = { active: true, public_id: token.actor, actor_type: 'carbon', client_id: 'ring', audience: 'ring', expires_at: Math.floor(Date.now() / 1000) + 3600,
        authorization: { organization_id: A, org_id: token.org, membership_id: `${token.actor}[${token.org}]`, membership_version: 1, authorization_epoch: 1, audience: 'ring', scopes: [], org_role: 'owner', actor_type: 'carbon', public_id: token.actor, testing_environment_id: realm === 'production' ? null : realm } };
      if (blockedIntrospection?.realm === realm && !blockedIntrospection.entered) {
        blockedIntrospection.entered = true;
        blockedIntrospection.release = () => { if (!response.destroyed) reply(response, 200, result); };
        return;
      }
      return reply(response, 200, result);
    }
    if (path === '/api/v1/me' && request.method === 'GET') {
      const token = tokens.get(request.headers.authorization?.replace(/^Bearer /, ''));
      if (!token || request.headers['x-testing-environment-key'] !== contexts.get(token.realm)?.key) return reject(response);
      return reply(response, 200, { display_name: token.actor });
    }
    unexpected.push(`${request.method} ${path}`); reply(response, 404, { error: { code: 'not_found', message: 'No fixture route' } });
  } catch (error) { unexpected.push(error.message); if (!response.headersSent) reply(response, 500, { error: { code: 'fixture_error', message: 'Fixture failed' } }); }
});
iam.listen(0, '127.0.0.1'); await once(iam, 'listening');
const reservation = createPortReservation(); reservation.listen(0, '127.0.0.1'); await once(reservation, 'listening');
const port = reservation.address().port; await new Promise(resolve => reservation.close(resolve));
const origin = `http://127.0.0.1:${port}`;
const env = Object.fromEntries(Object.entries(process.env).filter(([key]) => !/^(RING_|SILICON_RING_|SILICON_HOME$|SILICON_ORG$)/.test(key)));
Object.assign(env, { RING_BIND: `127.0.0.1:${port}`, RING_DATA_DIR: join(directory, 'data'), RING_ENV: 'development', RING_HONEYCOMB_CONTROL_TOKEN: controlToken,
  RING_IAM_URL: `http://127.0.0.1:${iam.address().port}`, RING_IAM_APP_SECRET: contexts.get('production').secret,
  RING_IAM_TEST_APP_SECRET: legacySecret, RING_IAM_TEST_ENVIRONMENT_KEY: 'L'.repeat(32), RING_TEST_APP_SECRET: legacySecret,
  RING_DISABLE_PROVIDERS: '1', RING_TELEMETRY_ENABLED: 'false', RING_TEST_TOKENS_FILE: join(directory, 'legacy-tokens.json'), RING_RELEASE_INDEX_FILE: join(directory, 'production-releases.json') });
await writeFile(env.RING_TEST_TOKENS_FILE, JSON.stringify({ legacy: { actor: 'c:alice', org_id: 'alice-org' } }), { mode: 0o600 });
// Metadata only: the harness never downloads or installs this synthetic release.
const globalRelease = { version: '9.9.9', protocol_major: 1, channel: 'stable', platform: 'linux', arch: 'x86_64', url: 'https://production-release.invalid/ring', sha256: '0'.repeat(64), signature: 'fixture-signature' };
await writeFile(env.RING_RELEASE_INDEX_FILE, JSON.stringify({ releases: [globalRelease] }), { mode: 0o600 });
async function start() {
  serverOutput = '';
  server = spawn(resolve('target/debug/ring-server'), [], { env, stdio: ['ignore', 'pipe', 'pipe'] });
  server.stdout.on('data', chunk => serverOutput += chunk); server.stderr.on('data', chunk => serverOutput += chunk);
  await waitFor(async () => { if (server.exitCode !== null) throw new Error(`Server exited: ${serverOutput}`); try { return (await fetch(`${origin}/health`)).ok; } catch { return false; } }, 'server startup');
}
async function stop() {
  for (const client of clients) client.close();
  if (server?.exitCode === null) {
    const exited = once(server, 'exit'); server.kill('SIGINT');
    const kill = setTimeout(() => server.kill('SIGKILL'), 4000); await exited; clearTimeout(kill);
  }
}
class Client {
  constructor(socket) {
    this.socket = socket; this.pending = new Map(); this.events = []; this.frames = []; this.closed = false;
    socket.addEventListener('message', ({ data }) => {
      const value = JSON.parse(data);
      if (this.pending.has(value.id)) {
        const { resolve, reject, timer } = this.pending.get(value.id); this.pending.delete(value.id); clearTimeout(timer);
        value.ok ? resolve(value.result) : reject(Object.assign(new Error(value.error.message), value.error));
      } else { this.events.push(value); if (value.type === 'media.audio') this.frames.push(value.data); }
    });
    socket.addEventListener('close', () => {
      this.closed = true;
      for (const { reject, timer } of this.pending.values()) { clearTimeout(timer); reject(Object.assign(new Error('Connection closed'), { code: 'CONNECTION_CLOSED' })); }
      this.pending.clear();
    });
  }
  request(method, params = {}, id = crypto.randomUUID()) {
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => { this.pending.delete(id); reject(new Error(`Timeout: ${method}`)); }, 8000);
      this.pending.set(id, { resolve, reject, timer }); this.socket.send(JSON.stringify({ id, method, params }));
    });
  }
  frame(type, data) { if (this.socket.readyState === WebSocket.OPEN) this.socket.send(JSON.stringify({ type, data })); }
  close() { if (this.socket.readyState === WebSocket.OPEN) this.socket.close(); }
}
async function socket() {
  const ws = new WebSocket(`ws://127.0.0.1:${port}/ws`); await once(ws, 'open');
  const client = new Client(ws); clients.push(client); return client;
}
async function hello(realm, who = 'alice', secret = contexts.get(realm)?.secret) {
  const client = await socket();
  const result = await client.request('protocol.hello', { versions: [1], realm, org_id: actors[who].org, ...(realm !== 'production' && secret !== undefined ? { test_app_secret: secret } : {}) });
  assert.equal(result.realm, realm); return client;
}
async function login(realm, who = 'alice') {
  const client = await hello(realm, who);
  client.session = await client.request('auth.login', { token: `${realm}|${who}` });
  assert.equal(client.session.realm, realm); assert.equal(client.session.org_id, actors[who].org); assert.equal(client.session.actor, actors[who].actor);
  await client.request('events.subscribe'); return client;
}
async function denied(operation, codes = ['FORBIDDEN', 'AUTH_REQUIRED', 'IAM_AUTH_FAILED', 'TEST_ENVIRONMENT_UNAVAILABLE']) {
  await assert.rejects(operation, error => codes.includes(error.code), `expected ${codes.join(' or ')}`);
}
function operation(realm, action, revision, generation = 1, keyVersion = 1) {
  return { operation_id: crypto.randomUUID(), environment_id: realm, org_id: 'test-owner', app_id: 'ring', environment_revision: revision, generation, key_version: keyVersion, action, testing_key: contexts.get(realm).key,
    ...(action === 'import' ? { snapshot: { app_id: 'ring', org_id: 'catalog-owner', source_revision: 2, source_iam_revision: 3, configuration_revision: 1, source_visibility: 'public', visibility: 'private', selected_release: '0.1.2', configuration: {} } } : {}) };
}
async function lifecycle(body, { bearer = controlToken, path = body, status = 200 } = {}) {
  const response = await fetch(`${origin}/internal/honeycomb/organizations/${path.org_id}/testing-environments/${path.environment_id}/operations/${path.operation_id}`, {
    method: 'PUT', headers: { 'content-type': 'application/json', ...(bearer ? { authorization: `Bearer ${bearer}` } : {}) }, body: JSON.stringify(body), signal: AbortSignal.timeout(10000),
  });
  const text = await response.text(); assert.equal(response.status, status, text);
  if (status !== 200) return text;
  const receipt = JSON.parse(text);
  for (const key of ['operation_id', 'environment_id', 'app_id', 'environment_revision', 'generation', 'key_version']) assert.deepEqual(receipt[key], body[key]);
  assert.equal(receipt.state, 'completed'); return receipt;
}
const pcm = Buffer.alloc(960); for (let n = 0; n < pcm.length; n += 2) pcm.writeInt16LE(1000, n);
const png = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+a5K0AAAAASUVORK5CYII=', 'base64');
async function active(realm) {
  const alice = await login(realm), bob = await login(realm, 'bob');
  const call = await alice.request('calls.init', { target: 'c:bob' });
  await bob.request('calls.accept', { ringid: call.ringid });
  const mic = await alice.request('media.attach', { ringid: call.ringid, device_id: alice.session.device_id, purpose: 'call' });
  const ears = await bob.request('media.attach', { ringid: call.ringid, device_id: bob.session.device_id, purpose: 'call' });
  let seq = 0;
  const feed = setInterval(() => {
    alice.frame('media.audio', { stream_id: mic.stream_id, seq, offset_ms: seq * 20, audio_base64: pcm.toString('base64') });
    bob.frame('media.audio', { stream_id: ears.stream_id, seq, offset_ms: seq++ * 20, audio_base64: Buffer.alloc(960).toString('base64') });
  }, 20); timers.push(feed);
  const audible = frames => frames.some(frame => Buffer.from(frame.audio_base64, 'base64').readInt16LE(0) === 1000);
  await waitFor(() => audible(bob.frames), `${realm} live PCM`);
  const upload = await alice.request('assets.begin', { purpose: 'profile_photo', mime_type: 'image/png', size_bytes: png.length });
  alice.frame('assets.chunk', { asset_id: upload.asset_id, seq: 0, data_base64: png.subarray(0, 24).toString('base64') });
  await waitFor(() => alice.events.some(event => event.type === 'assets.ack' && event.data.asset_id === upload.asset_id && event.data.received_bytes === 24), 'partial upload acknowledgement');
  return { realm, alice, bob, call, upload, feed, audible };
}
try {
  await start();
  const prepareA = operation(A, 'prepare', 1), prepareB = operation(B, 'prepare', 1);
  for (const bearer of [null, contexts.get(A).key, 'wrong-control-token']) await lifecycle(prepareA, { bearer, status: 401 });
  for (const [key, value] of [['org_id', 'different-owner'], ['environment_id', B], ['operation_id', crypto.randomUUID()]]) await lifecycle(prepareA, { path: { ...prepareA, [key]: value }, status: 400 });
  await lifecycle({ ...prepareA, app_id: 'ting' }, { status: 400 });
  await lifecycle({ ...prepareA, unexpected_field: true }, { status: 400 });
  const preparedReceipt = await lifecycle(prepareA); await lifecycle(prepareB);
  assert.deepEqual(await lifecycle(prepareA), preparedReceipt);
  await lifecycle({ ...prepareA, reason: 'changed retry' }, { status: 409 });
  await lifecycle(operation(A, 'import', 2)); await lifecycle(operation(B, 'import', 2));
  checked('dedicated lifecycle bearer, path identity, strict envelope and exact retry');

  for (const secret of ['', 'bad-secret', contexts.get(B).secret, contexts.get(A).key, legacySecret, 'wrong-ack']) await denied(() => hello(A, 'alice', secret));
  const missing = await socket(); await denied(() => missing.request('protocol.hello', { versions: [1], realm: A, org_id: 'alice-org' }));
  await denied(() => hello(B, 'alice', contexts.get(A).secret));
  await denied(() => hello(crypto.randomUUID(), 'alice', contexts.get(A).secret));
  const legacy = await hello('test', 'alice', legacySecret); assert.equal((await legacy.request('auth.login', { token: 'legacy' })).realm, 'test');
  checked('UUID hello validates current app credential and exact IAM realm; root/legacy credentials do not substitute');

  const a = await active(A), b = await active(B), production = await active('production');
  assert.equal(a.alice.session.actor, b.alice.session.actor); assert.equal(a.alice.session.actor, production.alice.session.actor);
  assert.notEqual(a.alice.session.org_id, a.bob.session.org_id);
  const releaseQuery = { current_version: '0.1.2', channel: 'stable', platform: 'linux', arch: 'x86_64' };
  for (const client of [production.alice, legacy]) {
    const release = await client.request('release.info', releaseQuery);
    assert.equal(release.update_available, true); assert.equal(release.url, globalRelease.url);
  }
  for (const scoped of [a, b]) {
    const release = await scoped.alice.request('release.info', releaseQuery);
    assert.equal(release.update_available, false); assert.equal(release.realm, scoped.realm);
    assert.equal(release.url, null); assert.equal(release.signature, null); assert.equal(release.managed_by, 'honeycomb');
  }
  checked('named environments never advertise the process-wide production update index');
  for (const source of [a, b, production]) {
    const resumed = await hello(source.realm);
    const result = await resumed.request('auth.resume', { session_token: source.alice.session.session_token, device_id: source.alice.session.device_id });
    assert.equal(result.realm, source.realm); assert.equal(result.org_id, 'alice-org');
    for (const destination of [a, b, production].filter(item => item !== source)) {
      const stranger = await hello(destination.realm);
      await denied(() => stranger.request('auth.resume', { session_token: source.alice.session.session_token, device_id: source.alice.session.device_id }));
      await denied(() => stranger.request('auth.login', { token: `${source.realm}|alice` }));
      for (const method of ['calls.get', 'transcript.list', 'events.subscribe']) await denied(() => destination.alice.request(method, { ringid: source.call.ringid }));
      await denied(() => destination.alice.request('media.attach', { ringid: source.call.ringid, device_id: destination.alice.session.device_id, purpose: 'call' }));
      await denied(() => destination.alice.request('assets.get', { asset_id: source.upload.asset_id }));
      assert(!(await destination.alice.request('calls.list')).items.some(call => call.ringid === source.call.ringid));
    }
  }
  const wrongOrg = await hello(A, 'bob'); await denied(() => wrongOrg.request('auth.login', { token: `${A}|alice` }));
  checked('same global actors make separate cross-org calls; login/resume, calls, transcript, events and uploads remain realm-bound');

  blockedIntrospection = { realm: A, entered: false };
  const pending = a.alice.request('config.set', { scope: 'actor', values: { 'representative.context_required': true } }).then(value => ({ value }), error => ({ error }));
  await waitFor(() => blockedIntrospection.entered, 'in-flight IAM control operation');
  const clean = operation(A, 'clean', 3, 2);
  await lifecycle(clean);
  await waitFor(() => a.alice.closed && a.bob.closed, 'clean closes managed WebSockets');
  assert.equal((await pending).error?.code, 'CONNECTION_CLOSED');
  blockedIntrospection.release(); clearInterval(a.feed);
  for (const untouched of [b, production]) {
    assert(!untouched.alice.closed); const frameStart = untouched.bob.frames.length;
    assert.equal((await untouched.alice.request('calls.get', { ringid: untouched.call.ringid })).state, 'active');
    assert.equal((await untouched.alice.request('assets.get', { asset_id: untouched.upload.asset_id })).received_bytes, 24);
    await waitFor(() => untouched.audible(untouched.bob.frames.slice(frameStart)), `${untouched.realm} PCM survives A clean`);
    untouched.alice.frame('assets.chunk', { asset_id: untouched.upload.asset_id, seq: 1, data_base64: png.subarray(24).toString('base64') });
    await waitFor(() => untouched.alice.events.some(event => event.type === 'assets.ack' && event.data.asset_id === untouched.upload.asset_id && event.data.received_bytes === png.length), 'unaffected upload resumes');
    assert.equal((await untouched.alice.request('assets.complete', { asset_id: untouched.upload.asset_id })).complete, true);
  }
  const cleaned = await login(A);
  await denied(() => cleaned.request('calls.get', { ringid: a.call.ringid }));
  await denied(() => cleaned.request('assets.get', { asset_id: a.upload.asset_id }));
  assert.deepEqual((await cleaned.request('calls.list')).items, []);
  assert.notEqual((await cleaned.request('config.get', { scope: 'actor' })).values['representative.context_required'], true);
  const expired = await hello(A); await denied(() => expired.request('auth.resume', { session_token: a.alice.session.session_token, device_id: a.alice.session.device_id }));
  checked('clean drains live WS/audio/upload and blocked control; B and production keep live audio, calls, uploads and sessions');

  await cleaned.request('config.set', { scope: 'actor', values: { 'representative.context_required': true } });
  await lifecycle(clean); // A replay must not clean newly written generation-2 state.
  assert.equal((await cleaned.request('config.get', { scope: 'actor' })).values['representative.context_required'], true);
  for (const timer of timers) clearInterval(timer);
  await stop(); await start();
  await lifecycle(clean); assert.deepEqual(await lifecycle(prepareA), preparedReceipt);
  const restarted = await login(A);
  assert.equal((await restarted.request('config.get', { scope: 'actor' })).values['representative.context_required'], true);
  const retainedB = await login(B), retainedProduction = await login('production');
  for (const [client, old] of [[retainedB, b], [retainedProduction, production]]) {
    assert.equal((await client.request('calls.get', { ringid: old.call.ringid })).state, 'ended');
    assert.equal((await client.request('assets.get', { asset_id: old.upload.asset_id })).received_bytes, png.length);
  }
  checked('operation receipts survive restart and exact clean retries preserve new-generation state');

  contexts.set(A, { key: 'C'.repeat(32), secret: 'ring-app-a-rotated' });
  const rotate = operation(A, 'rotate-key', 4, 2, 2); await lifecycle(rotate);
  await waitFor(() => restarted.closed, 'rotation closes old sessions');
  await denied(() => hello(A, 'alice', 'ring-app-a'));
  const rotated = await login(A); assert.equal((await rotated.request('config.get', { scope: 'actor' })).values['representative.context_required'], true);
  await lifecycle(operation(A, 'disable', 5, 2, 2)); await waitFor(() => rotated.closed, 'disable closes session');
  await denied(() => hello(A)); assert.equal((await retainedB.request('auth.status')).authenticated, true); assert.equal((await retainedProduction.request('auth.status')).authenticated, true);
  await lifecycle(operation(A, 'restore', 6, 2, 2)); const restored = await login(A);
  assert.equal((await restored.request('config.get', { scope: 'actor' })).values['representative.context_required'], true);
  const purge = operation(A, 'purge', 7, 2, 2); await lifecycle(purge); await lifecycle(purge);
  await denied(() => hello(A)); await lifecycle(operation(A, 'import', 8, 2, 2), { status: 409 });
  checked('rotation invalidates old credentials; disable/restore preserves data; purge stays unavailable and idempotent');

  assert.deepEqual(unexpected, []);
  for (const realm of [A, B, 'production']) assert(iamRequests.some(request => request.realm === realm && request.path === '/api/v1/oauth/introspect'));
  console.log(`PASS: ${checks} managed-environment transport checks`);
} finally {
  for (const timer of timers) clearInterval(timer);
  blockedIntrospection?.release?.();
  await stop(); iam.closeAllConnections(); await new Promise(resolve => iam.close(resolve));
  await rm(directory, { recursive: true, force: true });
}
