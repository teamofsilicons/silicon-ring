import { test } from 'node:test';
import assert from 'node:assert/strict';
import { CallAudio } from './audio.ts';
import { normalizeActor, normalizeRealm, bindSession, checkSessionContext, sessionToRestore, isActor, socketUrl, defaultSocketUrl, RingSocket, coalesceTranscript, readTranscriptSince, persistentStorage, carbonLogin, carbonCallback, nativeCarbonReturnUrl, nativeCarbonLink, nativeCarbonCallback, restoreCarbonExchange, type CarbonExchange, type Session, type RingError } from './protocol.ts';
test('ending a call while microphone permission is pending stops the late microphone', async t => {
  const original = Object.getOwnPropertyDescriptor(globalThis, 'navigator');
  t.after(() => { if (original) Object.defineProperty(globalThis, 'navigator', original); else Reflect.deleteProperty(globalThis, 'navigator'); });
  let grant!: (stream: MediaStream) => void, requested!: () => void, stopped = 0;
  const permissionRequested = new Promise<void>(resolve => { requested = resolve; });
  Object.defineProperty(globalThis, 'navigator', { configurable: true, value: { mediaDevices: { getUserMedia: () => { requested(); return new Promise<MediaStream>(resolve => { grant = resolve; }); } } } });
  const requests: string[] = [];
  const api = { ready: true, request: async (method: string) => { requests.push(method); return {}; } } as unknown as RingSocket;
  const audio = new CallAudio(), starting = audio.start(api, 'ring-pending', 'device');
  await permissionRequested; await audio.stop();
  grant({ getTracks: () => [{ stop: () => { stopped++; } }] } as unknown as MediaStream);
  await starting;
  assert.equal(stopped, 1); assert.equal(audio.streamId, ''); assert.deepEqual(requests, []);
});
test('ending a call during media attachment detaches its late stream without restarting capture', async t => {
  const originals = new Map(['navigator', 'AudioContext', 'AudioWorkletNode'].map(key => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  t.after(() => { for (const [key, original] of originals) { if (original) Object.defineProperty(globalThis, key, original); else Reflect.deleteProperty(globalThis, key); } });
  let attached!: (value: { stream_id: string }) => void, requested!: () => void, stopped = 0, closed = 0, worklets = 0;
  const attachmentRequested = new Promise<void>(resolve => { requested = resolve; });
  Object.defineProperty(globalThis, 'navigator', { configurable: true, value: { mediaDevices: { getUserMedia: async () => ({ getTracks: () => [{ stop: () => { stopped++; } }] }) } } });
  Object.defineProperty(globalThis, 'AudioContext', { configurable: true, value: class { audioWorklet = { addModule: async () => {} }; async resume() {} async close() { closed++; } } });
  Object.defineProperty(globalThis, 'AudioWorkletNode', { configurable: true, value: class { constructor() { worklets++; } } });
  const requests: Array<{ method: string; params: any }> = [];
  const api = { ready: true, request: async (method: string, params: any) => {
    requests.push({ method, params });
    if (method === 'media.attach') { requested(); return new Promise(resolve => { attached = resolve; }); }
    return {};
  } } as unknown as RingSocket;
  const audio = new CallAudio(), starting = audio.start(api, 'ring-pending', 'device');
  await attachmentRequested; await audio.stop(); attached({ stream_id: 'late-stream' }); await starting;
  assert.equal(stopped, 1); assert.equal(closed, 1); assert.equal(worklets, 0); assert.equal(audio.streamId, '');
  assert.deepEqual(requests.map(item => item.method), ['media.attach', 'media.detach']);
  assert.deepEqual(requests[1].params, { stream_id: 'late-stream' });
});
test('packaged native clients use production while local web and development clients stay local', () => {
  assert.equal(defaultSocketUrl('localhost', true, false), 'wss://backend.ring.teamofsilicons.com/ws');
  assert.equal(defaultSocketUrl('localhost', true, true), 'ws://127.0.0.1:8765/ws');
  assert.equal(defaultSocketUrl('127.0.0.1', false, false), 'ws://127.0.0.1:8765/ws');
  assert.equal(defaultSocketUrl('localhost', false, true), 'ws://127.0.0.1:8765/ws');
  assert.equal(defaultSocketUrl('ring.teamofsilicons.com', false, false), 'wss://backend.ring.teamofsilicons.com/ws');
});
test('call targets normalize to global public IDs without accepting malformed IDs', () => {
  assert.equal(normalizeActor(' @c:alex '), 'c:alex');
  assert.equal(normalizeActor(' @c:alex[another-org] '), 'c:alex');
  assert.equal(normalizeActor('si:assistant[any-org]'), 'si:assistant');
  for (const id of ['si:assistant', '@c:alex', 'c:alex[another-org]']) assert.equal(isActor(id), true);
  for (const id of ['', 'alex', 'si:', 'c:alex smith', 'c:a[team][other]', 'c:alex[]', 'c:alex[team space]', 'c:alex[[team]]', 'c:alex[team]extra']) assert.equal(isActor(id), false);
  assert.throws(() => socketUrl('https://example.com'));
  assert.throws(() => socketUrl('wss://user:password@example.com/ws'));
  assert.throws(() => socketUrl('ws://example.com/ws'));
  assert.equal(socketUrl('ws://[::1]:8765/ws'), 'ws://[::1]:8765/ws');
  assert.equal(socketUrl('ws://127.0.0.1:8765/ws'), 'ws://127.0.0.1:8765/ws');
});
test('offline mutations fail explicitly instead of disappearing', async () => {
  await assert.rejects(new RingSocket().request('calls.init', { target: 'c:alex' }), /offline/);
});

test('browser sessions migrate from tab storage and survive a new tab until logout', () => {
  const memory = () => { const values = new Map<string, string>(); return { getItem: (key: string) => values.get(key) || null, setItem: (key: string, value: string) => { values.set(key, value); }, removeItem: (key: string) => { values.delete(key); } }; };
  const local = memory(), tab = memory();
  const session: Session = { actor: 'c:a', org_id: 'team', device_id: 'device', session_token: 'opaque-ring-session', url: 'wss://example.com/ws', realm: 'production' };
  tab.setItem('ring.session', JSON.stringify(session));
  const storage = persistentStorage(local, tab);
  assert.deepEqual(storage.read('ring.session', null), session);
  assert.equal(tab.getItem('ring.session'), null);
  assert.deepEqual(persistentStorage(local, memory()).read('ring.session', null), session);
  const renewed = { ...session, expires_at: '2030-01-01T00:00:00Z' };
  storage.write('ring.session', renewed);
  assert.deepEqual(persistentStorage(local, memory()).read('ring.session', null), renewed);
  tab.setItem('ring.session', JSON.stringify(session));
  storage.remove('ring.session');
  assert.equal(storage.read('ring.session', null), null);
  assert.equal(tab.getItem('ring.session'), null);
  local.setItem('ring.session', 'invalid-json');
  assert.equal(storage.read('ring.session', null), null);
  local.removeItem('ring.session'); tab.setItem('ring.session', JSON.stringify(session));
  assert.deepEqual(persistentStorage({ ...local, setItem: () => { throw new Error('Storage full'); } }, tab).read('ring.session', null), session);
});

test('Carbon login uses the IAM redirect contract and accepts only a fresh callback from this tab', () => {
  const connection = { url: 'wss://example.com/ws', realm: 'production', org_id: '' };
  const { url, pending } = carbonLogin('https://iam.teamofsilicons.com/login', 'ring', 'https://ring.example/?view=calls#history', connection, 1000);
  const login = new URL(url), callback = new URL(login.searchParams.get('redirect_uri')!);
  assert.equal(login.pathname, '/login'); assert.equal(login.searchParams.get('app_id'), 'ring');
  assert.equal(login.searchParams.get('identity_kind'), 'carbon');
  assert.equal(login.searchParams.has('org_id'), false); assert.equal(login.searchParams.has('state'), false);
  assert.equal(callback.searchParams.get('ring_auth_state'), pending.state);
  assert.equal(callback.hash, '');
  callback.searchParams.set('slt', 'one-use-iam-token');
  const completed = carbonCallback(callback.toString(), pending, 1001)!;
  assert.equal(completed.token, 'one-use-iam-token'); assert.deepEqual(completed.connection, connection);
  assert.equal(completed.cleanUrl, 'https://ring.example/?view=calls');
  assert.equal(carbonCallback(completed.cleanUrl, null), null);
  for (const saved of [null, { ...pending, state: 'different' }, { ...pending, return_url: 'https://other.example/' }, { ...pending, expires_at: 1001 }, { ...pending, expires_at: 9999999 }]) {
    const denied = carbonCallback(callback.toString(), saved, 1001)!;
    assert.ok(denied.error); assert.equal(denied.token, undefined); assert.equal(denied.cleanUrl, completed.cleanUrl);
  }
  callback.searchParams.append('slt', 'duplicate');
  assert.ok(carbonCallback(callback.toString(), pending, 1001)?.error);
  callback.searchParams.set('slt', '');
  assert.ok(carbonCallback(callback.toString(), pending, 1001)?.error);
  callback.searchParams.set('error', 'rejected'); callback.searchParams.set('error_description', 'untrusted-content');
  const denied = carbonCallback(callback.toString(), pending, 1001)!;
  assert.ok(denied.error); assert.equal(denied.error.includes('untrusted-content'), false); assert.equal(denied.cleanUrl, completed.cleanUrl);
  for (const insecure of ['http://ring.example/', 'https://user:password@ring.example/', 'javascript:alert(1)']) {
    assert.throws(() => carbonLogin(insecure, 'ring', pending.return_url, connection), /HTTPS/);
    assert.throws(() => carbonLogin(login.toString(), 'ring', insecure, connection), /HTTPS/);
  }
  assert.doesNotThrow(() => carbonLogin('http://localhost:3000/login', 'ring', 'http://localhost:1420/', connection));
});

test('native Carbon callbacks return through a fixed HTTPS bridge and require the saved one-use state', () => {
  const connection = { url: 'wss://example.com/ws', realm: 'production' };
  const login = carbonLogin('https://iam.teamofsilicons.com/login', 'ring', nativeCarbonReturnUrl, connection, 1000);
  const callback = new URL(new URL(login.url).searchParams.get('redirect_uri')!);
  callback.searchParams.set('slt', 'one-use-native-token');
  const link = nativeCarbonLink(callback.toString())!;
  assert.equal(new URL(link).origin, 'null');
  assert.ok(link.startsWith('silicon-ring://login/callback?'));
  const durablePending = JSON.parse(JSON.stringify(login.pending));
  const result = nativeCarbonCallback(link, durablePending, 1001)!;
  assert.equal(result.token, 'one-use-native-token');
  assert.deepEqual(result.connection, connection);
  assert.equal(result.cleanUrl, nativeCarbonReturnUrl);
  for (const pending of [null, { ...durablePending, state: 'different' }, { ...durablePending, expires_at: 1001 }, { ...durablePending, return_url: 'https://other.example/native-login.html' }]) assert.ok(nativeCarbonCallback(link, pending, 1001)?.error);
  for (const invalid of [link.replace('silicon-ring:', 'other:'), link.replace('//login/', '//other/'), link.replace('/callback?', '/other?'), link + '#token', link + '&next=https://other.example/', 'not a URL']) assert.equal(nativeCarbonCallback(invalid, durablePending, 1001), null);
  for (const invalid of [callback.toString().replace('ring.teamofsilicons.com', 'other.example'), callback.toString().replace('/native-login.html', '/other'), callback.toString() + '&slt=duplicate', callback.toString() + '&next=silicon-ring://other', callback.toString() + '#fragment']) assert.equal(nativeCarbonLink(invalid), null);
  callback.searchParams.delete('slt'); callback.searchParams.set('error', 'denied'); callback.searchParams.set('error_description', 'untrusted provider description');
  const denied = nativeCarbonLink(callback.toString())!;
  assert.ok(denied); assert.equal(denied.includes('description'), false);
  assert.ok(nativeCarbonCallback(denied, durablePending, 1001)?.error);
  callback.searchParams.delete('error');
  assert.equal(nativeCarbonLink(callback.toString()), null);
});

test('uncertain Carbon exchanges recover the exact request and connection only during the two-minute tab window', () => {
  const pending: CarbonExchange = { token: 'one-use-token', id: 'original-request', expected_actor_type: 'carbon', expires_at: 121000, connection: { url: 'wss://example.com/ws', realm: 'production', org_id: 'team' } };
  const restored = restoreCarbonExchange(JSON.parse(JSON.stringify(pending)), 1000)!;
  assert.deepEqual(restored, pending); assert.equal(restored.id, 'original-request');
  for (const invalid of [null, { ...pending, id: '' }, { ...pending, token: '' }, { ...pending, expected_actor_type: 'silicon' }, { ...pending, expires_at: 1000 }, { ...pending, expires_at: 121001 }, { ...pending, connection: { ...pending.connection, url: 'ws://remote.example/ws' } }, { ...pending, connection: { ...pending.connection, realm: 'invalid' } }]) {
    assert.equal(restoreCarbonExchange(invalid as CarbonExchange | null, 1000), null);
  }
  assert.equal(restoreCarbonExchange(pending, pending.expires_at), null);
});

test('environment UUIDs are canonical and cached sessions stay bound to their connection', () => {
  const realm = 'be0137a4-7901-4199-ae21-5a6ccf497a15';
  const settings = { url: 'wss://example.com/ws', realm, org_id: 'team' };
  const session = { actor: 'c:a', org_id: 'team', device_id: 'device', session_token: 'token', realm };
  assert.equal(normalizeRealm(` ${realm.toUpperCase()} `), realm);
  for (const invalid of ['', 'custom', '00000000-0000-0000-0000-000000000000', '1-1-1-1-1']) assert.throws(() => normalizeRealm(invalid), /UUID/);
  const bound = bindSession(session, settings);
  assert.equal(bound.realm, realm); assert.equal(bound.url, settings.url);
  checkSessionContext(bound, settings);
  for (const changed of [{ ...settings, realm: 'test' }, { ...settings, url: 'wss://other.example/ws' }, { ...settings, org_id: 'other' }]) assert.throws(() => checkSessionContext(bound, changed), /different connection settings/);
  assert.throws(() => checkSessionContext(session, settings), /different connection settings/);
  assert.throws(() => bindSession({ ...session, realm: undefined }, settings), /confirm the selected environment/);
  assert.throws(() => bindSession({ ...session, realm: 'test' }, settings), /confirm the selected environment/);
  for (const realm of ['production', 'test']) assert.equal(bindSession({ ...session, realm: undefined }, { ...settings, realm }).realm, realm);
});

test('WebSocket authentication waits for the selected environment and rejects cross-environment resumes', async () => {
  const realm = 'be0137a4-7901-4199-ae21-5a6ccf497a15';
  const settings = { url: 'wss://example.com/ws', realm, org_id: 'team', test_app_secret: 'test-secret' };
  const session: Session = { actor: 'c:a', org_id: 'team', device_id: 'device', session_token: 'token', realm, url: settings.url };
  const original = globalThis.WebSocket;
  let hello: any = {}, resumed: any = session, requests: any[] = [], opened = 0, resumeError: RingError | undefined;
  class FakeWebSocket {
    static OPEN = 1;
    readyState = 1;
    onopen?: () => void;
    onclose?: () => void;
    onmessage?: (message: { data: string }) => void;
    constructor() { opened++; queueMicrotask(() => this.onopen?.()); }
    send(value: string) {
      const request = JSON.parse(value); requests.push(request);
      const response = request.method === 'auth.resume' && resumeError ? { id: request.id, ok: false, error: resumeError } : { id: request.id, ok: true, result: request.method === 'protocol.hello' ? hello : request.method === 'auth.resume' ? resumed : {} };
      queueMicrotask(() => this.onmessage?.({ data: JSON.stringify(response) }));
    }
    close() { this.readyState = 3; this.onclose?.(); }
  }
  globalThis.WebSocket = FakeWebSocket as unknown as typeof WebSocket;
  try {
    for (hello of [{}, { realm: 'test' }, { realm: 'production' }, { realm: '77777777-7777-4777-a777-777777777777' }, { realm: null }]) {
      for (const cached of [undefined, session]) {
        requests = [];
        const api = new RingSocket(); api.session = cached;
        await assert.rejects(api.connect(settings).then(() => api.request('auth.login', { token: 'iam-token' })), /confirm the selected environment/);
        assert.deepEqual(requests.map(item => item.method), ['protocol.hello']);
        assert.equal(api.ready, false); api.disconnect();
      }
    }
    hello = { realm }; requests = [];
    const api = new RingSocket(); api.session = session;
    const connected = api.connect({ ...settings, realm: realm.toUpperCase() });
    await assert.rejects(api.connect({ ...settings, realm: 'test' }), /current connection uses different settings/);
    await assert.rejects(api.request('auth.login', { token: 'too-early' }), /offline/);
    await connected;
    assert.deepEqual(requests.map(item => item.method), ['protocol.hello', 'auth.resume', 'events.subscribe']);
    assert.equal(requests[0].params.realm, realm); assert.equal(api.session?.realm, realm);
    const before = opened;
    await api.connect(settings);
    assert.equal(opened, before);
    for (const changed of [{ ...settings, realm: 'test' }, { ...settings, url: 'wss://other.example/ws' }, { ...settings, org_id: 'other' }, { ...settings, test_app_secret: 'other-secret' }]) await assert.rejects(api.connect(changed), /current connection uses different settings/);
    assert.equal(api.ready, true); assert.equal(api.session?.realm, realm); assert.equal(opened, before);
    api.disconnect();
    for (resumed of [{ ...session, realm: 'test' }, { ...session, realm: undefined }]) {
      requests = [];
      const api = new RingSocket(); api.session = session;
      await assert.rejects(api.connect(settings), /confirm the selected environment/);
      assert.deepEqual(requests.map(item => item.method), ['protocol.hello', 'auth.resume']);
      assert.equal(api.session, undefined); api.disconnect();
    }
    for (const changed of [{ ...settings, realm: 'test' }, { ...settings, url: 'wss://other.example/ws' }, { ...settings, org_id: 'other' }]) {
      const api = new RingSocket(); api.session = session; const before = opened;
      await assert.rejects(api.connect(changed), /different connection settings/);
      assert.equal(opened, before);
    }
    for (const realm of ['test', settings.realm]) {
      const before = opened;
      await assert.rejects(new RingSocket().connect({ ...settings, realm, test_app_secret: ' ' }), /secret is required/);
      assert.equal(opened, before);
    }
    for (const realm of ['production', 'test']) {
      hello = {}; requests = []; resumed = { ...session, realm: undefined };
      const api = new RingSocket(); api.session = { ...session, realm };
      await api.connect({ ...settings, realm });
      assert.equal(api.session?.realm, realm);
      assert.equal(requests[0].params.test_app_secret, realm === 'production' ? undefined : 'test-secret');
      api.disconnect();
      hello = { realm: settings.realm };
      await assert.rejects(new RingSocket().connect({ ...settings, realm }), /confirm the selected environment/);
    }
    hello = { realm }; resumed = { ...session, expires_at: '2030-01-01T00:00:00Z' };
    const renewed = new RingSocket(); renewed.session = session;
    let saved: Session | undefined;
    renewed.onSession = value => { saved = value; };
    await renewed.connect(settings); assert.equal(saved?.expires_at, resumed.expires_at); renewed.disconnect();
    for (const [code, retryable, terminal] of [['IAM_UNAVAILABLE', true, false], ['AUTH_UNAVAILABLE', true, false], ['IAM_AUTH_FAILED', true, false], ['AUTH_REQUIRED', false, true], ['IAM_AUTH_FAILED', false, true], ['IAM_LOGIN_REQUIRED', false, true]] as const) {
      resumeError = { code, retryable, message: 'Resume failed' };
      const api = new RingSocket(); api.session = session;
      let expired = 0; api.onExpired = () => { expired++; };
      await assert.rejects(api.connect(settings), (error: any) => error.code === code && error.retryable === retryable);
      assert.equal(expired, terminal ? 1 : 0);
      assert.equal(api.session, terminal ? undefined : session);
      api.disconnect();
    }
  } finally { globalThis.WebSocket = original; }
});

test('native session bindings migrate only the matching browser cache and respect explicit settings', () => {
  const saved: Session = { actor: 'c:a', org_id: 'team', device_id: 'device', session_token: 'token' };
  const native = { ...saved, realm: 'test', url: 'wss://example.com/ws' };
  assert.equal(sessionToRestore(null, native), native);
  assert.equal(sessionToRestore(saved, native), native);
  assert.equal(sessionToRestore({ ...saved, realm: 'test' }, native), native);
  assert.equal(sessionToRestore(saved, null), saved);
  assert.equal(sessionToRestore(native, { ...native }), native);
  for (const different of [{ ...native, session_token: 'other' }, { ...native, device_id: 'other' }, { ...native, url: undefined }, { ...native, realm: undefined }]) assert.equal(sessionToRestore(saved, different), saved);
  const anotherRealm = { ...saved, realm: 'production' };
  assert.equal(sessionToRestore(anotherRealm, native), anotherRealm);
  const restored = sessionToRestore(saved, native)!;
  checkSessionContext(restored, { url: native.url, realm: native.realm, org_id: 'team' });
  for (const changed of [{ url: native.url, realm: 'production' }, { url: 'wss://other.example/ws', realm: native.realm }, { url: native.url, realm: native.realm, org_id: 'other' }]) assert.throws(() => checkSessionContext(restored, changed), /different connection settings/);
});

test('live transcript revisions replace interim text without repeating a segment', () => {
  const rows = coalesceTranscript([
    { seq: 1, kind: 'participant.joined' },
    { seq: 2, kind: 'speech', actor: 'c:a', data: { segment_id: 's1', revision: 1, text: 'Good' } },
    { seq: 3, kind: 'speech', actor: 'c:a', data: { segment_id: 's1', revision: 2, text: 'Good morning', final: true } },
    { seq: 4, kind: 'speech', actor: 'c:b', data: { segment_id: 's2', revision: 1, text: 'Hello' } },
  ]);
  assert.equal(rows.length, 3);
  assert.equal(rows[1].data.text, 'Good morning');
  assert.equal(rows[2].data.text, 'Hello');
});

test('transcripts load every page and keep earlier history when later revisions arrive', async () => {
  const entries = Array.from({ length: 451 }, (_, i) => ({ seq: i + 1, kind: 'speech', actor: 'c:a', data: { segment_id: `s${i}`, revision: 1, text: `Words ${i}` } }));
  const requests: any[] = [];
  let head = 460; // Some private entries at the tail are invisible to this viewer.
  const api = { request: async (method: string, params: any): Promise<any> => {
    assert.equal(method, 'transcript.list'); requests.push(params);
    const rows = entries.filter(row => row.seq > params.after_seq), offset = Number((params.cursor || 'offset:0').split(':')[1]);
    return { items: rows.slice(offset, offset + params.limit), latest_seq: head, next_cursor: offset + params.limit < rows.length ? `offset:${offset + params.limit}` : null };
  } };
  const first = await readTranscriptSince(api, 'call-1');
  assert.equal(first.items.length, 451);
  assert.equal(first.latest_seq, 460);
  assert.deepEqual(requests.map(p => [p.after_seq, p.cursor]), [[0, undefined], [0, 'offset:200'], [0, 'offset:400']]);
  entries.push({ seq: 461, kind: 'speech', actor: 'c:a', data: { segment_id: 's0', revision: 2, text: 'Final words' } });
  entries.push({ seq: 462, kind: 'speech', actor: 'c:a', data: { segment_id: 's451', revision: 1, text: 'A new sentence' } });
  head = 462;
  const next = await readTranscriptSince(api, 'call-1', first.latest_seq);
  const shown = coalesceTranscript([...first.items, ...next.items]);
  assert.equal(requests.at(-1).after_seq, 460);
  assert.equal(shown.length, 452);
  assert.equal(shown[0].data.text, 'Final words');
  assert.equal(shown.at(-1).data.text, 'A new sentence');
});

test('transcript pagination fails explicitly without committing a partial checkpoint', async () => {
  let count = 0;
  await assert.rejects(readTranscriptSince({ request: async (): Promise<any> => {
    if (++count === 2) throw new Error('Connection lost');
    return { items: [{ seq: 1 }], latest_seq: 250, next_cursor: 'offset:200' };
  } }, 'call-1'), /Connection lost/);
  assert.equal(count, 2);
  await assert.rejects(readTranscriptSince({ request: async (): Promise<any> => ({ items: [], latest_seq: 250, next_cursor: 'offset:200' }) }, 'call-1'), /repeated a transcript cursor/);
});

import { readFileSync } from 'node:fs';
import { runInNewContext } from 'node:vm';
test('microphone worklet emits 20 ms PCM frames at 24 kHz from common hardware sample rates', () => {
  for (const hardwareRate of [24000, 44100, 48000]) {
    const emitted: { buffer: ArrayBuffer; rms: number }[] = [];
    let Processor: any;
    runInNewContext(readFileSync(new URL('../public/pcm-worklet.js', import.meta.url), 'utf8'), {
      sampleRate: hardwareRate,
      AudioWorkletProcessor: class { port = { postMessage: (value: any) => emitted.push(value) }; },
      registerProcessor: (_name: string, implementation: any) => { Processor = implementation; },
      Int16Array, Math,
    });
    const processor = new Processor();
    for (let offset = 0; offset < hardwareRate; offset += 128) processor.process([[new Float32Array(Math.min(128, hardwareRate - offset)).fill(0.25)]]);
    assert.ok(emitted.length >= 49 && emitted.length <= 50, `one second at ${hardwareRate} Hz buffers at most one 20 ms frame`);
    for (const frame of emitted) { assert.equal(frame.buffer.byteLength, 960); assert.equal(new Int16Array(frame.buffer)[0], 8192); assert.ok(Math.abs(frame.rms - 0.25) < 0.001); }
  }
});
