import { test } from 'node:test';
import assert from 'node:assert/strict';
import { normalizeActor, isActor, socketUrl, defaultSocketUrl, RingSocket, coalesceTranscript } from './protocol.ts';
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
