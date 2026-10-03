import { test } from 'node:test';
import assert from 'node:assert/strict';
import { normalizeActor, normalizeRealm, bindSession, checkSessionContext, sessionToRestore, isActor, socketUrl, defaultSocketUrl, RingSocket, coalesceTranscript, readTranscriptSince, type Session } from './protocol.ts';
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
  let hello: any = {}, resumed: any = session, requests: any[] = [], opened = 0;
  class FakeWebSocket {
    static OPEN = 1;
    readyState = 1;
    onopen?: () => void;
    onclose?: () => void;
    onmessage?: (message: { data: string }) => void;
    constructor() { opened++; queueMicrotask(() => this.onopen?.()); }
    send(value: string) {
      const request = JSON.parse(value); requests.push(request);
      queueMicrotask(() => this.onmessage?.({ data: JSON.stringify({ id: request.id, ok: true, result: request.method === 'protocol.hello' ? hello : request.method === 'auth.resume' ? resumed : {} }) }));
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
