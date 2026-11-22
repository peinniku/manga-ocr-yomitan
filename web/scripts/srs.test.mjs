// The wasm scheduler must reproduce the native Rust result for the shared fixture.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';

import { preview, replay, useExports } from '../src/srs.js';

const here = dirname(fileURLToPath(import.meta.url));
const fixture = JSON.parse(readFileSync(join(here, '../../crates/srs/tests/fixture.json'), 'utf8'));
const { instance } = await WebAssembly.instantiate(readFileSync(join(here, '../public/srs.wasm')), {});
useExports(instance.exports);

test('wasm replay matches native schedule', () => {
  const s = replay(fixture.logs);
  const e = fixture.expected;
  assert.equal(s.phase, e.phase);
  assert.equal(s.reps, e.reps);
  assert.equal(s.lapses, e.lapses);
  // f32 maths may differ in the last bit between platforms: allow a second of drift.
  assert.ok(Math.abs(s.due - e.due) < 1000, `due ${s.due} vs ${e.due}`);
  assert.ok(Math.abs(s.stability - e.stability) < 1e-3);
  assert.ok(Math.abs(s.difficulty - e.difficulty) < 1e-3);
});

test('replay ignores log order and preview gives four increasing waits', () => {
  const a = replay(fixture.logs);
  const b = replay([...fixture.logs].reverse());
  assert.deepEqual(a, b);
  const p = preview(a, a.due);
  assert.equal(p.due.length, 4);
  assert.ok(p.due[0] < p.due[2] && p.due[2] <= p.due[3]);
  assert.equal(p.labels.length, 4);
});
